//! Bounded adapter for the documented Herdr JSON CLI operations.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::SidebarError;
use super::layout::{PaneRect, ResizeDirection, SplitDirection};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

pub(crate) struct Layout {
    pub(crate) area_width: u64,
    pub(crate) rects: Vec<PaneRect>,
}

#[allow(async_fn_in_trait)]
pub(crate) trait HerdrApi {
    async fn live_panes(&self) -> Result<HashSet<String>, SidebarError>;
    async fn layout(&self, pane: &str) -> Result<Layout, SidebarError>;
    async fn create_parking_tab(&self, workspace: &str) -> Result<(String, String), SidebarError>;
    async fn move_pane(
        &self,
        pane: &str,
        tab: &str,
        direction: SplitDirection,
        target: Option<&str>,
        ratio: Option<f64>,
    ) -> Result<(), SidebarError>;
    async fn open_sidebar(
        &self,
        target: &str,
        state_file: &OsStr,
        token: &str,
    ) -> Result<String, SidebarError>;
    async fn close_plugin_pane(&self, pane: &str) -> Result<(), SidebarError>;
    async fn close_pane(&self, pane: &str) -> Result<(), SidebarError>;
    async fn resize_pane(
        &self,
        pane: &str,
        direction: ResizeDirection,
        amount: f64,
    ) -> Result<(), SidebarError>;
}

#[allow(async_fn_in_trait)]
pub(crate) trait CommandRunner {
    async fn run(&self, arguments: &[OsString]) -> Result<Value, SidebarError>;
}

pub(crate) struct ProcessRunner {
    bin: OsString,
}

impl ProcessRunner {
    fn from_environment() -> Self {
        Self {
            bin: std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| OsString::from("herdr")),
        }
    }
}

impl CommandRunner for ProcessRunner {
    async fn run(&self, arguments: &[OsString]) -> Result<Value, SidebarError> {
        let mut command = Command::new(&self.bin);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|_| SidebarError::Api)?;
        let stdout = child.stdout.take().ok_or(SidebarError::Api)?;
        let output = tokio::time::timeout(COMMAND_TIMEOUT, async move {
            let mut bytes = Vec::new();
            stdout
                .take((MAX_RESPONSE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| SidebarError::Api)?;
            if bytes.len() > MAX_RESPONSE_BYTES {
                child.kill().await.ok();
                child.wait().await.ok();
                return Err(SidebarError::Api);
            }
            let status = child.wait().await.map_err(|_| SidebarError::Api)?;
            if !status.success() {
                return Err(SidebarError::Api);
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| SidebarError::Api)??;
        serde_json::from_slice(&output).map_err(|_| SidebarError::Api)
    }
}

pub(crate) struct CliHerdrApi<R = ProcessRunner> {
    runner: R,
}

impl CliHerdrApi<ProcessRunner> {
    pub(crate) fn from_environment() -> Self {
        Self {
            runner: ProcessRunner::from_environment(),
        }
    }
}

impl<R: CommandRunner> CliHerdrApi<R> {
    async fn run(&self, arguments: &[&str]) -> Result<Value, SidebarError> {
        self.runner
            .run(&arguments.iter().map(OsString::from).collect::<Vec<_>>())
            .await
    }

    async fn run_owned(&self, arguments: Vec<OsString>) -> Result<Value, SidebarError> {
        self.runner.run(&arguments).await
    }
}

impl<R: CommandRunner> HerdrApi for CliHerdrApi<R> {
    async fn live_panes(&self) -> Result<HashSet<String>, SidebarError> {
        let value = self.run(&["pane", "list"]).await?;
        let panes = value
            .get("result")
            .and_then(|value| value.get("panes"))
            .and_then(Value::as_array)
            .ok_or(SidebarError::Api)?;
        panes
            .iter()
            .map(|pane| string_at(pane, &["pane_id"]).map(ToOwned::to_owned))
            .collect()
    }

    async fn layout(&self, pane: &str) -> Result<Layout, SidebarError> {
        let value = self.run(&["pane", "layout", "--pane", pane]).await?;
        let layout = value
            .get("result")
            .and_then(|value| value.get("layout"))
            .ok_or(SidebarError::Api)?;
        let panes = layout
            .get("panes")
            .and_then(Value::as_array)
            .ok_or(SidebarError::Api)?;
        let origin_x = integer_at(layout, &["area", "x"])?;
        let origin_y = integer_at(layout, &["area", "y"])?;
        let area_width = unsigned_at(layout, &["area", "width"])?;
        let rects = panes
            .iter()
            .map(|pane| {
                let x = integer_at(pane, &["rect", "x"])?
                    .checked_sub(origin_x)
                    .and_then(|value| u64::try_from(value).ok())
                    .ok_or(SidebarError::Api)?;
                let y = integer_at(pane, &["rect", "y"])?
                    .checked_sub(origin_y)
                    .and_then(|value| u64::try_from(value).ok())
                    .ok_or(SidebarError::Api)?;
                Ok(PaneRect {
                    pane_id: string_at(pane, &["pane_id"])?.to_owned(),
                    x,
                    y,
                    width: unsigned_at(pane, &["rect", "width"])?,
                    height: unsigned_at(pane, &["rect", "height"])?,
                })
            })
            .collect::<Result<Vec<_>, SidebarError>>()?;
        Ok(Layout { area_width, rects })
    }

    async fn create_parking_tab(&self, workspace: &str) -> Result<(String, String), SidebarError> {
        let value = self
            .run(&["tab", "create", "--workspace", workspace, "--no-focus"])
            .await?;
        Ok((
            string_at(&value, &["result", "tab", "tab_id"])?.to_owned(),
            string_at(&value, &["result", "root_pane", "pane_id"])?.to_owned(),
        ))
    }

    async fn move_pane(
        &self,
        pane: &str,
        tab: &str,
        direction: SplitDirection,
        target: Option<&str>,
        ratio: Option<f64>,
    ) -> Result<(), SidebarError> {
        let mut arguments = [
            "pane",
            "move",
            pane,
            "--tab",
            tab,
            "--split",
            direction.as_str(),
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
        if let Some(target) = target {
            arguments.extend([OsString::from("--target-pane"), OsString::from(target)]);
        }
        if let Some(ratio) = ratio {
            arguments.extend([OsString::from("--ratio"), OsString::from(ratio.to_string())]);
        }
        arguments.push(OsString::from("--no-focus"));
        self.run_owned(arguments).await.map(|_| ())
    }

    async fn open_sidebar(
        &self,
        target: &str,
        state_file: &OsStr,
        token: &str,
    ) -> Result<String, SidebarError> {
        let mut arguments = [
            "plugin",
            "pane",
            "open",
            "--plugin",
            "herdr-quota",
            "--entrypoint",
            "dashboard",
            "--placement",
            "split",
            "--target-pane",
            target,
            "--direction",
            "right",
            "--env",
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
        let mut state_environment = OsString::from("HERDR_QUOTA_STATE_FILE=");
        state_environment.push(state_file);
        arguments.push(state_environment);
        arguments.extend([
            OsString::from("--env"),
            OsString::from(format!("HERDR_QUOTA_STATE_TOKEN={token}")),
            OsString::from("--focus"),
        ]);
        let value = self.run_owned(arguments).await?;
        Ok(string_at(&value, &["result", "plugin_pane", "pane", "pane_id"])?.to_owned())
    }

    async fn close_plugin_pane(&self, pane: &str) -> Result<(), SidebarError> {
        self.run(&["plugin", "pane", "close", pane])
            .await
            .map(|_| ())
    }

    async fn close_pane(&self, pane: &str) -> Result<(), SidebarError> {
        self.run(&["pane", "close", pane]).await.map(|_| ())
    }

    async fn resize_pane(
        &self,
        pane: &str,
        direction: ResizeDirection,
        amount: f64,
    ) -> Result<(), SidebarError> {
        self.run_owned(
            [
                OsString::from("pane"),
                OsString::from("resize"),
                OsString::from("--pane"),
                OsString::from(pane),
                OsString::from("--direction"),
                OsString::from(direction.as_str()),
                OsString::from("--amount"),
                OsString::from(amount.to_string()),
            ]
            .into(),
        )
        .await
        .map(|_| ())
    }
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a Value, SidebarError> {
    path.iter().try_fold(value, |current, key| {
        current.get(key).ok_or(SidebarError::Api)
    })
}

fn string_at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str, SidebarError> {
    value_at(value, path)?.as_str().ok_or(SidebarError::Api)
}

fn integer_at(value: &Value, path: &[&str]) -> Result<i64, SidebarError> {
    value_at(value, path)?.as_i64().ok_or(SidebarError::Api)
}

fn unsigned_at(value: &Value, path: &[&str]) -> Result<u64, SidebarError> {
    value_at(value, path)?.as_u64().ok_or(SidebarError::Api)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct FakeRunner {
        arguments: Mutex<Vec<Vec<String>>>,
        responses: Mutex<VecDeque<Value>>,
    }

    impl FakeRunner {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                arguments: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into()),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        async fn run(&self, arguments: &[OsString]) -> Result<Value, SidebarError> {
            self.arguments.lock().expect("arguments").push(
                arguments
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
            );
            self.responses
                .lock()
                .expect("responses")
                .pop_front()
                .ok_or(SidebarError::Api)
        }
    }

    fn ok() -> Value {
        serde_json::json!({"result": {}})
    }

    #[tokio::test]
    async fn mutation_commands_preserve_documented_focus_and_placement_flags() {
        let runner = FakeRunner::new(vec![
            serde_json::json!({
                "result": {"tab": {"tab_id": "parking"}, "root_pane": {"pane_id": "placeholder"}}
            }),
            ok(),
            serde_json::json!({
                "result": {"plugin_pane": {"pane": {"pane_id": "sidebar"}}}
            }),
            ok(),
            ok(),
            ok(),
            ok(),
        ]);
        let api = CliHerdrApi { runner };

        api.create_parking_tab("workspace")
            .await
            .expect("parking tab");
        api.move_pane("p2", "parking", SplitDirection::Right, None, None)
            .await
            .expect("evacuate");
        api.open_sidebar("p1", OsStr::new("/tmp/state.json"), "token-1")
            .await
            .expect("open sidebar");
        api.move_pane("p2", "tab", SplitDirection::Down, Some("p1"), Some(0.4))
            .await
            .expect("restore");
        api.resize_pane("sidebar", ResizeDirection::Right, 0.2)
            .await
            .expect("resize");
        api.close_plugin_pane("sidebar")
            .await
            .expect("close plugin pane");
        api.close_pane("placeholder")
            .await
            .expect("close placeholder pane");

        let calls = api.runner.arguments.lock().expect("arguments");
        assert_eq!(
            calls[0],
            ["tab", "create", "--workspace", "workspace", "--no-focus"]
        );
        assert_eq!(
            calls[1],
            [
                "pane",
                "move",
                "p2",
                "--tab",
                "parking",
                "--split",
                "right",
                "--no-focus"
            ]
        );
        assert!(calls[2].ends_with(&[
            "--env".into(),
            "HERDR_QUOTA_STATE_TOKEN=token-1".into(),
            "--focus".into()
        ]));
        assert_eq!(
            calls[3],
            [
                "pane",
                "move",
                "p2",
                "--tab",
                "tab",
                "--split",
                "down",
                "--target-pane",
                "p1",
                "--ratio",
                "0.4",
                "--no-focus",
            ]
        );
        assert_eq!(
            calls[4],
            [
                "pane",
                "resize",
                "--pane",
                "sidebar",
                "--direction",
                "right",
                "--amount",
                "0.2"
            ]
        );
        assert_eq!(calls[5], ["plugin", "pane", "close", "sidebar"]);
        assert_eq!(calls[6], ["pane", "close", "placeholder"]);
    }

    #[tokio::test]
    async fn layout_normalizes_the_reported_area_origin() {
        let runner = FakeRunner::new(vec![serde_json::json!({
            "result": {"layout": {
                "area": {"x": 5, "y": 7, "width": 120},
                "panes": [{"pane_id": "p1", "rect": {"x": 5, "y": 7, "width": 48, "height": 30}}]
            }}
        })]);
        let api = CliHerdrApi { runner };

        let layout = api.layout("p1").await.expect("layout");

        assert_eq!(layout.area_width, 120);
        assert_eq!(
            layout.rects[0],
            PaneRect {
                pane_id: "p1".to_owned(),
                x: 0,
                y: 0,
                width: 48,
                height: 30,
            }
        );
    }
}
