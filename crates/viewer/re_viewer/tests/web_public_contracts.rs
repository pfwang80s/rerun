#![cfg(feature = "testing")]

use re_build_info::CrateVersion;
use re_log_channel::{LogSource, SmartMessagePayload, log_channel};
use re_log_encoding::{DecoderApp, Encoder, EncodingOptions};
use re_log_types::{LogMsg, SetStoreInfo, StoreId, StoreInfo, StoreSource};
use re_viewer::viewer_test_utils::{AppTestingExt as _, HarnessOptions, viewer_harness};
use re_viewer_context::{Route, SystemCommand};

fn set_store_info(store_id: StoreId) -> LogMsg {
    LogMsg::SetStoreInfo(SetStoreInfo {
        row_id: *re_chunk::RowId::new(),
        info: StoreInfo::new(store_id, StoreSource::Unknown),
    })
}

#[tokio::test]
async fn compatibility_open_and_select_uses_completion_order() {
    let mut harness = viewer_harness(&HarnessOptions::default());
    let source_a = LogSource::HttpStream {
        url: "https://example.test/a.rrd".to_owned(),
    };
    let source_b = LogSource::HttpStream {
        url: "https://example.test/b.rrd".to_owned(),
    };
    let (tx_a, rx_a) = log_channel(source_a);
    let (tx_b, rx_b) = log_channel(source_b);
    let store_a = StoreId::recording("completion-order", "a");
    let store_b = StoreId::recording("completion-order", "b");

    harness.state_mut().add_log_receiver(rx_a);
    harness.state_mut().add_log_receiver(rx_b);

    tx_b.send(set_store_info(store_b.clone()).into()).unwrap();
    harness.step();
    assert_eq!(harness.state().active_recording_id(), Some(&store_b));

    tx_a.send(set_store_info(store_a.clone()).into()).unwrap();
    harness.step();
    assert_eq!(harness.state().active_recording_id(), Some(&store_a));
    assert_eq!(
        harness.state().testonly_get_route(),
        &Route::LocalRecording {
            recording_id: store_a,
        }
    );
}

#[tokio::test]
async fn one_compatibility_receiver_selects_each_store_it_discovers() {
    let opened_recordings = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let callback_recordings = opened_recordings.clone();
    let mut harness = viewer_harness(&HarnessOptions {
        on_event: Some(std::rc::Rc::new(move |event| {
            if matches!(event.kind, re_viewer::ViewerEventKind::RecordingOpen { .. }) {
                callback_recordings
                    .borrow_mut()
                    .push(event.recording_id.to_string());
            }
        })),
        ..Default::default()
    });
    let source = LogSource::HttpStream {
        url: "https://example.test/multi-store.rrd".to_owned(),
    };
    let (tx, rx) = log_channel(source);
    let first = StoreId::recording("multi-store", "first");
    let second = StoreId::recording("multi-store", "second");

    harness.state_mut().add_log_receiver(rx);

    tx.send(set_store_info(first.clone()).into()).unwrap();
    harness.step();
    assert_eq!(harness.state().active_recording_id(), Some(&first));

    tx.send(set_store_info(second.clone()).into()).unwrap();
    harness.step();
    assert_eq!(harness.state().active_recording_id(), Some(&second));

    let hub = harness.state_mut().testonly_get_store_hub();
    assert!(hub.entity_db(&first).is_some());
    assert!(hub.entity_db(&second).is_some());
    assert_eq!(
        &*opened_recordings.borrow(),
        &[
            first.recording_id().to_string(),
            second.recording_id().to_string()
        ]
    );
}

#[tokio::test]
async fn panel_close_removes_http_receiver_and_store() {
    let mut harness = viewer_harness(&HarnessOptions::default());
    let source = LogSource::HttpStream {
        url: "https://example.test/close-me.rrd".to_owned(),
    };
    let (tx, rx) = log_channel(source.clone());
    let store_id = StoreId::recording("close-source", "recording");

    harness.state_mut().add_log_receiver(rx);
    tx.send(set_store_info(store_id.clone()).into()).unwrap();
    harness.step();
    assert!(harness.state().testonly_has_log_source(&source));
    assert!(
        harness
            .state_mut()
            .testonly_get_store_hub()
            .entity_db(&store_id)
            .is_some()
    );

    harness
        .state()
        .testonly_send_system_command(SystemCommand::CloseRecordingOrTable(
            store_id.clone().into(),
        ));
    harness.step();

    assert!(!harness.state().testonly_has_log_source(&source));
    assert!(
        harness
            .state_mut()
            .testonly_get_store_hub()
            .entity_db(&store_id)
            .is_none()
    );
}

#[tokio::test]
async fn malformed_compatibility_item_does_not_block_the_next_item() {
    let mut harness = viewer_harness(&HarnessOptions::default());

    harness.state().open_url_or_file("not a URL");
    harness.state().open_url_or_file("about:settings");
    harness.step();

    assert!(matches!(
        harness.state().testonly_get_route(),
        Route::Settings { .. }
    ));
}

#[tokio::test]
async fn startup_url_is_dispatched_before_the_initial_stable_state() {
    let harness = viewer_harness(&HarnessOptions {
        startup_url: Some("about:settings".to_owned()),
        ..Default::default()
    });

    assert!(matches!(
        harness.state().testonly_get_route(),
        Route::Settings { .. }
    ));
}

#[tokio::test]
async fn encoded_rrd_reaches_viewer_through_a_real_log_channel() {
    let store_id = StoreId::recording("js-channel", "decoded");
    let message = set_store_info(store_id.clone());
    let mut encoder = Encoder::new_eager(
        CrateVersion::LOCAL,
        EncodingOptions::PROTOBUF_UNCOMPRESSED,
        Vec::new(),
    )
    .unwrap();
    encoder.append(&message).unwrap();
    let bytes = encoder.into_inner().unwrap();

    let source = LogSource::JsChannel {
        channel_name: "characterization".to_owned(),
    };
    let (tx, rx) = log_channel(source.clone());
    for decoded in DecoderApp::decode_eager(std::io::Cursor::new(bytes)).unwrap() {
        tx.send(decoded.unwrap().into()).unwrap();
    }

    let mut harness = viewer_harness(&HarnessOptions::default());
    harness.state_mut().add_log_receiver(rx);
    assert!(harness.state().testonly_has_log_source(&source));
    harness.step();
    assert!(
        harness
            .state_mut()
            .testonly_get_store_hub()
            .entity_db(&store_id)
            .is_some()
    );

    tx.quit(None).unwrap();
    let (_, quit) = harness
        .state_mut()
        .msg_receive_set()
        .try_recv()
        .expect("quit marker should reach the real receiver");
    assert!(matches!(quit.payload, SmartMessagePayload::Quit(None)));
}
