use std::sync::Arc;

use ipc::ConnectionAddress;
use warp::integration_testing::{
    pane_group::assert_num_panes_in_tab,
    step::new_step_with_default_assertions,
    terminal::wait_until_bootstrapped_single_pane_for_tab,
};
use warpui::integration::TestStep;

use super::new_builder;
use crate::Builder;

/// Integration test for the `remote_control` IPC feature.
///
/// Boots Warp, sends a Ping over the IPC channel, then sends a
/// `SplitActivePaneAndRun` request and waits for the pane count to reach 2.
pub fn test_remote_control_split_and_run() -> Builder {
    new_builder()
        // Step 1: wait until the single pane in tab 0 has bootstrapped.
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        // Step 2: connect an IPC client, send Ping → assert Pong, then send
        //         SplitActivePaneAndRun → assert Ok.
        .with_step(
            TestStep::new("remote_control: ping and split via IPC")
                .with_action(|app, _window_id, _step_data| {
                    let background_executor = app.background_executor();

                    warpui::r#async::block_on(async move {
                        // Read the socket address published by the server.
                        let addr_path = warp::remote_control::socket_address_path()
                            .expect("remote_control addr path should be available");
                        let addr_str = std::fs::read_to_string(&addr_path).unwrap_or_else(|e| {
                            panic!(
                                "Failed to read remote_control address file at {}: {e}",
                                addr_path.display()
                            )
                        });
                        let addr_str = addr_str.trim().to_string();

                        // Connect an IPC client.
                        let client = Arc::new(
                            ipc::Client::connect(
                                ConnectionAddress::from(addr_str),
                                background_executor,
                            )
                            .await
                            .expect("IPC client should connect to remote_control server"),
                        );

                        let caller = ipc::service_caller::<
                            warp::remote_control::RemoteControlService,
                        >(client);

                        // --- Ping → Pong ---
                        let ping_resp = caller
                            .call(warp::remote_control::RemoteControlRequest::Ping)
                            .await
                            .expect("Ping call should succeed");
                        assert!(
                            matches!(
                                ping_resp,
                                warp::remote_control::RemoteControlResponse::Pong
                            ),
                            "Expected Pong, got {ping_resp:?}",
                        );

                        // --- SplitActivePaneAndRun → Ok ---
                        let split_resp = caller
                            .call(
                                warp::remote_control::RemoteControlRequest::SplitActivePaneAndRun {
                                    command: "echo hello-from-remote-control".to_string(),
                                    direction: warp::remote_control::SplitDirection::Right,
                                },
                            )
                            .await
                            .expect("SplitActivePaneAndRun call should succeed");
                        assert!(
                            matches!(
                                split_resp,
                                warp::remote_control::RemoteControlResponse::Ok
                            ),
                            "Expected Ok, got {split_resp:?}",
                        );
                    });
                }),
        )
        // Step 3: wait for the split to materialise — pane count in tab 0 should be 2.
        .with_step(
            new_step_with_default_assertions("remote_control: pane count should be 2 after split")
                .add_assertion(assert_num_panes_in_tab(0, 2)),
        )
}
