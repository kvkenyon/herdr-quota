//! Sidebar toggle orchestration.

pub(crate) mod api;
pub(crate) mod layout;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use api::{CliHerdrApi, HerdrApi};
use layout::{SplitDirection, plan_rebuild, resize_for_target};
use serde_json::Value;
use store::sidebar_state::{
    FileSidebarStore, SIDEBAR_SCHEMA_VERSION, SidebarPhase, SidebarState, SidebarStateError,
    SidebarStore, runtime_state_path,
};

use crate::store;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SidebarError {
    #[error("AI Quota requires an active Herdr pane")]
    MissingContext,
    #[error("Herdr returned an unusable sidebar response")]
    Api,
    #[error("the current pane layout cannot be rebuilt safely")]
    UnsafeLayout,
    #[error("sidebar coordination state is unavailable")]
    State,
}

impl From<SidebarStateError> for SidebarError {
    fn from(_: SidebarStateError) -> Self {
        Self::State
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SidebarContext {
    workspace: String,
    tab: String,
    focused_pane: String,
    state_file: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToggleOutcome {
    Opened,
    Closed,
}

/// Run the action entrypoint using only the current Herdr process context.
pub async fn run_from_environment() -> Result<(), SidebarError> {
    let context = context_from_environment()?;
    let store = FileSidebarStore::new(&context.state_file);
    toggle_sidebar(
        &CliHerdrApi::from_environment(),
        &store,
        &context,
        &ownership_token()?,
    )
    .await
    .map(|_| ())
}

fn ownership_token() -> Result<String, SidebarError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| SidebarError::State)?;
    Ok(bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}

async fn toggle_sidebar<A: HerdrApi, S: SidebarStore>(
    api: &A,
    store: &S,
    context: &SidebarContext,
    token: &str,
) -> Result<ToggleOutcome, SidebarError> {
    if let Some(existing) = store.load()? {
        if existing.tab == context.tab && existing.phase == SidebarPhase::Open {
            let live = api.live_panes().await?;
            if let Some(sidebar) = existing
                .sidebar_pane
                .as_ref()
                .filter(|pane| live.contains(*pane))
            {
                api.close_plugin_pane(sidebar).await?;
                remove_owned(store, &existing.token)?;
                return Ok(ToggleOutcome::Closed);
            }
            remove_owned(store, &existing.token)?;
        } else if existing.tab == context.tab {
            restore_evacuation(api, store, existing).await?;
        } else {
            remove_owned(store, &existing.token)?;
        }
    }

    let original = api.layout(&context.focused_pane).await?;
    let plan = plan_rebuild(&original.rects)?;
    let mut state = SidebarState {
        schema_version: SIDEBAR_SCHEMA_VERSION,
        phase: SidebarPhase::Evacuating,
        token: token.to_owned(),
        workspace: context.workspace.clone(),
        tab: context.tab.clone(),
        original_focus: context.focused_pane.clone(),
        plan,
        parked: Vec::new(),
        parking_placeholder: None,
        sidebar_pane: None,
    };
    store.save(&state)?;

    let opened = async {
        if original.rects.len() > 1 {
            let (tab, placeholder) = api.create_parking_tab(&context.workspace).await?;
            state.parking_placeholder = Some(placeholder);
            store.save(&state)?;
            for rect in &original.rects {
                if rect.pane_id == state.plan.anchor {
                    continue;
                }
                api.move_pane(&rect.pane_id, &tab, SplitDirection::Right, None, None)
                    .await?;
                state.parked.push(rect.pane_id.clone());
                store.save(&state)?;
            }
        }

        let sidebar = api
            .open_sidebar(&state.plan.anchor, context.state_file.as_os_str(), token)
            .await?;
        state.sidebar_pane = Some(sidebar.clone());
        store.save(&state)?;

        for step in &state.plan.steps {
            api.move_pane(
                &step.pane,
                &context.tab,
                step.direction,
                Some(&step.target),
                Some(step.ratio),
            )
            .await?;
            state.parked.retain(|pane| pane != &step.pane);
            store.save(&state)?;
        }

        if let Some(placeholder) = state.parking_placeholder.take() {
            api.close_pane(&placeholder).await?;
            store.save(&state)?;
        }

        let current = api.layout(&sidebar).await?;
        let sidebar_rect = current
            .rects
            .iter()
            .find(|rect| rect.pane_id == sidebar)
            .ok_or(SidebarError::Api)?;
        if let Some((direction, amount)) = resize_for_target(current.area_width, sidebar_rect.width)
        {
            api.resize_pane(&sidebar, direction, amount).await?;
        }

        state.phase = SidebarPhase::Open;
        store.save(&state)?;
        Ok(ToggleOutcome::Opened)
    }
    .await;

    if let Err(error) = opened {
        let _ = restore_evacuation(api, store, state).await;
        return Err(error);
    }
    opened
}

async fn restore_evacuation<A: HerdrApi, S: SidebarStore>(
    api: &A,
    store: &S,
    mut state: SidebarState,
) -> Result<(), SidebarError> {
    let mut live = api.live_panes().await?;
    if let Some(sidebar) = state
        .sidebar_pane
        .as_ref()
        .filter(|pane| live.contains(*pane))
    {
        api.close_plugin_pane(sidebar).await?;
        live = api.live_panes().await?;
    }

    for step in state.plan.steps.clone() {
        if !state.parked.contains(&step.pane) {
            continue;
        }
        if !live.contains(&step.pane) {
            state.parked.retain(|pane| pane != &step.pane);
            store.save(&state)?;
            continue;
        }
        if !live.contains(&step.target) {
            return Err(SidebarError::UnsafeLayout);
        }
        api.move_pane(
            &step.pane,
            &state.tab,
            step.direction,
            Some(&step.target),
            Some(step.ratio),
        )
        .await?;
        state.parked.retain(|pane| pane != &step.pane);
        store.save(&state)?;
        live.insert(step.pane);
    }

    if let Some(placeholder) = state
        .parking_placeholder
        .as_ref()
        .filter(|pane| live.contains(*pane))
    {
        api.close_pane(placeholder).await?;
    }
    remove_owned(store, &state.token)
}

fn remove_owned<S: SidebarStore>(store: &S, token: &str) -> Result<(), SidebarError> {
    if store.remove_owned(token)? {
        Ok(())
    } else {
        Err(SidebarStateError::ForeignOwner.into())
    }
}

fn context_from_environment() -> Result<SidebarContext, SidebarError> {
    let plugin_context = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
        .unwrap_or(Value::Null);
    let workspace = environment_or_context("HERDR_WORKSPACE_ID", &plugin_context, "workspace_id")?;
    let tab = environment_or_context("HERDR_TAB_ID", &plugin_context, "tab_id")?;
    let focused_pane = environment_or_context("HERDR_PANE_ID", &plugin_context, "focused_pane_id")?;
    let session = std::env::var_os("HERDR_SOCKET_PATH")
        .as_deref()
        .and_then(|path| Path::new(path).parent())
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("HERDR_SESSION").ok())
        .unwrap_or_else(|| "local".to_owned());
    Ok(SidebarContext {
        state_file: runtime_state_path(&session, &tab),
        workspace,
        tab,
        focused_pane,
    })
}

fn environment_or_context(
    environment: &str,
    context: &Value,
    field: &str,
) -> Result<String, SidebarError> {
    std::env::var(environment)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            context
                .get(field)
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .filter(|value| !value.is_empty())
        .ok_or(SidebarError::MissingContext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar::api::Layout;
    use crate::sidebar::layout::{PaneRect, ResizeDirection};
    use std::cell::RefCell;
    use std::collections::HashSet;

    #[derive(Default)]
    struct MemoryStore(RefCell<Option<SidebarState>>);

    impl SidebarStore for MemoryStore {
        fn load(&self) -> Result<Option<SidebarState>, SidebarStateError> {
            Ok(self.0.borrow().clone())
        }

        fn save(&self, state: &SidebarState) -> Result<(), SidebarStateError> {
            *self.0.borrow_mut() = Some(state.clone());
            Ok(())
        }

        fn remove_owned(&self, token: &str) -> Result<bool, SidebarStateError> {
            let mut state = self.0.borrow_mut();
            if state.is_none() || state.as_ref().is_some_and(|state| state.token == token) {
                *state = None;
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }

    struct FakeApi {
        operations: RefCell<Vec<String>>,
        live: RefCell<HashSet<String>>,
        fail_once: RefCell<Option<String>>,
    }

    impl FakeApi {
        fn new() -> Self {
            Self {
                operations: RefCell::new(Vec::new()),
                live: RefCell::new(
                    ["p1", "p2", "p3", "sidebar", "placeholder"]
                        .into_iter()
                        .map(ToOwned::to_owned)
                        .collect(),
                ),
                fail_once: RefCell::new(None),
            }
        }

        fn record(&self, operation: String) -> Result<(), SidebarError> {
            self.operations.borrow_mut().push(operation.clone());
            if self.fail_once.borrow().as_deref() == Some(operation.as_str()) {
                self.fail_once.borrow_mut().take();
                Err(SidebarError::Api)
            } else {
                Ok(())
            }
        }
    }

    impl HerdrApi for FakeApi {
        async fn live_panes(&self) -> Result<HashSet<String>, SidebarError> {
            self.record("live".to_owned())?;
            Ok(self.live.borrow().clone())
        }

        async fn layout(&self, pane: &str) -> Result<Layout, SidebarError> {
            self.record(format!("layout {pane}"))?;
            if pane == "p2" {
                Ok(Layout {
                    area_width: 120,
                    rects: vec![
                        rect("p1", 0, 0, 48, 100),
                        rect("p2", 48, 0, 72, 50),
                        rect("p3", 48, 50, 72, 50),
                    ],
                })
            } else {
                Ok(Layout {
                    area_width: 120,
                    rects: vec![
                        rect("p1", 0, 0, 24, 100),
                        rect("p2", 24, 0, 36, 50),
                        rect("p3", 24, 50, 36, 50),
                        rect("sidebar", 60, 0, 60, 100),
                    ],
                })
            }
        }

        async fn create_parking_tab(
            &self,
            workspace: &str,
        ) -> Result<(String, String), SidebarError> {
            self.record(format!("create-tab {workspace}"))?;
            Ok(("parking".to_owned(), "placeholder".to_owned()))
        }

        async fn move_pane(
            &self,
            pane: &str,
            tab: &str,
            direction: SplitDirection,
            target: Option<&str>,
            ratio: Option<f64>,
        ) -> Result<(), SidebarError> {
            self.record(format!(
                "move {pane} -> {tab} {}{}",
                direction.as_str(),
                target
                    .zip(ratio)
                    .map(|(target, ratio)| format!(" {target} {ratio}"))
                    .unwrap_or_default()
            ))
        }

        async fn open_sidebar(
            &self,
            target: &str,
            _state_file: &OsStr,
            token: &str,
        ) -> Result<String, SidebarError> {
            self.record(format!("open {target} right focus {token}"))?;
            Ok("sidebar".to_owned())
        }

        async fn close_plugin_pane(&self, pane: &str) -> Result<(), SidebarError> {
            self.record(format!("close-plugin {pane}"))?;
            self.live.borrow_mut().remove(pane);
            Ok(())
        }

        async fn close_pane(&self, pane: &str) -> Result<(), SidebarError> {
            self.record(format!("close-pane {pane}"))
        }

        async fn resize_pane(
            &self,
            pane: &str,
            direction: ResizeDirection,
            amount: f64,
        ) -> Result<(), SidebarError> {
            self.record(format!("resize {pane} {} {amount}", direction.as_str()))
        }
    }

    fn rect(id: &str, x: u64, y: u64, width: u64, height: u64) -> PaneRect {
        PaneRect {
            pane_id: id.to_owned(),
            x,
            y,
            width,
            height,
        }
    }

    fn context() -> SidebarContext {
        SidebarContext {
            workspace: "w1".to_owned(),
            tab: "t1".to_owned(),
            focused_pane: "p2".to_owned(),
            state_file: PathBuf::from("/tmp/quota-t1.json"),
        }
    }

    #[tokio::test]
    async fn open_and_close_preserve_operation_and_focus_order() {
        let api = FakeApi::new();
        let store = MemoryStore::default();

        assert_eq!(
            toggle_sidebar(&api, &store, &context(), "token-1").await,
            Ok(ToggleOutcome::Opened)
        );
        assert_eq!(
            api.operations.borrow().as_slice(),
            [
                "layout p2",
                "create-tab w1",
                "move p2 -> parking right",
                "move p3 -> parking right",
                "open p1 right focus token-1",
                "move p2 -> t1 right p1 0.4",
                "move p3 -> t1 down p2 0.5",
                "close-pane placeholder",
                "layout sidebar",
                "resize sidebar right 0.2",
            ]
        );
        assert_eq!(
            store.0.borrow().as_ref().expect("open state").parked,
            Vec::<String>::new()
        );

        api.operations.borrow_mut().clear();
        assert_eq!(
            toggle_sidebar(&api, &store, &context(), "token-2").await,
            Ok(ToggleOutcome::Closed)
        );
        assert_eq!(
            api.operations.borrow().as_slice(),
            ["live", "close-plugin sidebar"]
        );
        assert!(store.0.borrow().is_none());
    }

    #[tokio::test]
    async fn failed_rebuild_closes_the_focused_sidebar_before_no_focus_restore() {
        let api = FakeApi::new();
        let store = MemoryStore::default();
        *api.fail_once.borrow_mut() = Some("move p3 -> t1 down p2 0.5".to_owned());

        assert_eq!(
            toggle_sidebar(&api, &store, &context(), "token-1").await,
            Err(SidebarError::Api)
        );
        let operations = api.operations.borrow();
        let rollback = operations
            .iter()
            .position(|operation| operation == "live")
            .expect("rollback begins");
        assert_eq!(
            &operations[rollback..],
            [
                "live",
                "close-plugin sidebar",
                "live",
                "move p3 -> t1 down p2 0.5",
                "close-pane placeholder",
            ]
        );
        assert!(store.0.borrow().is_none());
    }

    #[tokio::test]
    async fn corrupt_or_future_state_prevents_all_api_mutation() {
        struct FailingStore(SidebarStateError);
        impl SidebarStore for FailingStore {
            fn load(&self) -> Result<Option<SidebarState>, SidebarStateError> {
                Err(match self.0 {
                    SidebarStateError::Corrupt => SidebarStateError::Corrupt,
                    _ => SidebarStateError::Incompatible,
                })
            }
            fn save(&self, _: &SidebarState) -> Result<(), SidebarStateError> {
                unreachable!()
            }
            fn remove_owned(&self, _: &str) -> Result<bool, SidebarStateError> {
                unreachable!()
            }
        }

        for error in [SidebarStateError::Corrupt, SidebarStateError::Incompatible] {
            let api = FakeApi::new();
            assert!(
                toggle_sidebar(&api, &FailingStore(error), &context(), "token")
                    .await
                    .is_err()
            );
            assert!(api.operations.borrow().is_empty());
        }
    }
}
