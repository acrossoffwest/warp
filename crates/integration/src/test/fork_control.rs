use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::tab::assert_tab_title;
use warp::integration_testing::terminal::wait_until_bootstrapped_single_pane_for_tab;
use warpui_core::async_assert;
use warpui_core::integration::TestStep;

use super::new_builder;
use crate::Builder;

fn socket_path() -> PathBuf {
    std::env::temp_dir()
        .join(format!("wfc-{}", std::process::id()))
        .join("control.sock")
}

/// Drives the fork control API from a background thread (the UI thread must stay
/// free to answer) and checks the results once they arrive.
pub fn test_fork_control_api() -> Builder {
    let socket = socket_path();
    // SAFETY: runs before the app starts, while the test process is single-threaded.
    unsafe { std::env::set_var(warp_fork_control::paths::SOCKET_ENV, &socket) };
    let results: Arc<Mutex<Option<Vec<Value>>>> = Arc::default();
    let writer = results.clone();
    let reader = results.clone();
    let cwd = std::env::temp_dir();

    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(TestStep::new("fork_control: drive the API").with_action(
            move |_app, _window_id, _step_data| {
                let socket = socket.clone();
                let writer = writer.clone();
                let cwd = cwd.clone();
                std::thread::spawn(move || {
                    let call = |method: &str, params: Value| {
                        warp_fork_control::client::call(&socket, method, params)
                            .expect("fork control call should succeed")
                    };
                    let ping = call("ping", json!({}));
                    let list = call("list", json!({}));
                    let first_pane = list["result"]["panes"][0]["pane_id"].clone();
                    let open = call(
                        "open_tab",
                        json!({"cwd": cwd, "title": "FC-TEST", "command": "echo fork-control"}),
                    );
                    let at_prompt = call("send_input", json!({"pane_id": first_pane, "text": "x"}));
                    let missing = call("focus", json!({"pane_id": 999_999_999u64}));
                    *writer.lock().unwrap() = Some(vec![ping, list, open, at_prompt, missing]);
                });
            },
        ))
        .with_step(
            new_step_with_default_assertions("fork_control: results").add_assertion(
                move |_app, _window_id| {
                    let guard = reader.lock().unwrap();
                    let Some(results) = guard.as_ref() else {
                        return async_assert!(false, "waiting for fork control results");
                    };
                    let [ping, list, open, at_prompt, missing] = results.as_slice() else {
                        return async_assert!(false, "expected 5 results");
                    };
                    async_assert!(
                        ping["ok"] == json!(true)
                            && ping["api_version"] == json!(1)
                            && list["result"]["panes"]
                                .as_array()
                                .is_some_and(|p| p.len() == 1)
                            && list["result"]["panes"][0]["shell_pid"].is_u64()
                            && open["ok"] == json!(true)
                            && open["result"]["pane_id"].is_u64()
                            && at_prompt["error"]["code"] == json!("not_in_tui")
                            && missing["error"]["code"] == json!("not_found"),
                        "unexpected results: {results:?}"
                    )
                },
            ),
        )
        .with_step(
            new_step_with_default_assertions("fork_control: new tab has the custom title")
                .add_assertion(assert_tab_title(1, "FC-TEST")),
        )
}
