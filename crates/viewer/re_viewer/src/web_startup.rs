use re_log_channel::RecordingOpenBehavior;
use re_viewer_context::{CommandSender, open_url};

use re_viewer_context::open_url::ViewerOpenUrl;

/// Decision returned by the production-disarmed remote-MCAP singleton seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompatibilityRemoteMcapControlV1 {
    ExistingDispatcher,
    RemoteAccepted,
    RemoteSessionLimitReached,
}

/// Outcome of one compatibility URL dispatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompatibilityUrlDispatchOutcomeV1 {
    ExistingDispatcher,
    RemoteAccepted,
    RemoteSessionLimitReached,
}

/// Strict HTTP-only route accepted by the production-disarmed Wasm boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StrictHttpRemoteMcapRouteV1 {
    ExplicitMcap,
    ExtensionlessSniff,
}

/// Fixed, secret-free strict route validation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StrictHttpRemoteMcapRouteErrorV1 {
    InvalidUrl,
    UnsupportedStrictOpenRoute,
    UnsupportedFormat,
}

impl StrictHttpRemoteMcapRouteErrorV1 {
    #[cfg(target_arch = "wasm32")]
    pub(crate) const fn wire_code_v1(self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalid_url",
            Self::UnsupportedStrictOpenRoute => "unsupported_strict_open_route",
            Self::UnsupportedFormat => "unsupported_format",
        }
    }
}

/// Classifies one strict URL without creating any receiver, connection, Fetch, or Store effect.
pub(crate) fn classify_strict_http_remote_mcap_route_v1(
    raw_url: &str,
    allow_extensionless_sniff: bool,
) -> Result<StrictHttpRemoteMcapRouteV1, StrictHttpRemoteMcapRouteErrorV1> {
    let url = raw_url
        .parse::<url::Url>()
        .map_err(|_error| StrictHttpRemoteMcapRouteErrorV1::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(StrictHttpRemoteMcapRouteErrorV1::UnsupportedStrictOpenRoute);
    }
    if url.username() != "" || url.password().is_some() {
        return Err(StrictHttpRemoteMcapRouteErrorV1::UnsupportedStrictOpenRoute);
    }

    let last_segment = url.path().rsplit('/').next().unwrap_or_default();
    let extension = last_segment
        .rsplit_once('.')
        .map(|(_stem, extension)| extension);
    if extension.is_some_and(|extension| extension.eq_ignore_ascii_case("mcap")) {
        return Ok(StrictHttpRemoteMcapRouteV1::ExplicitMcap);
    }
    if extension.is_some() || !allow_extensionless_sniff {
        return Err(StrictHttpRemoteMcapRouteErrorV1::UnsupportedFormat);
    }
    Ok(StrictHttpRemoteMcapRouteV1::ExtensionlessSniff)
}

/// Parses and dispatches one compatibility URL through the exact production `WebHandle` path.
///
/// The callback is invoked only for an explicit HTTP(S) `.mcap` URL.  Returning
/// `ExistingDispatcher` from the callback immediately uses the existing `ViewerOpenUrl::open`
/// command path, which is the production-disarmed fallback.
pub(crate) fn dispatch_compatibility_url_v1(
    raw_url: &str,
    egui_ctx: &egui::Context,
    command_sender: &CommandSender,
    remote_dispatch: impl FnOnce() -> CompatibilityRemoteMcapControlV1,
) -> anyhow::Result<CompatibilityUrlDispatchOutcomeV1> {
    let parsed = raw_url.parse::<ViewerOpenUrl>()?;
    let is_explicit_remote_mcap = match &parsed {
        ViewerOpenUrl::HttpUrl(url) => url
            .path()
            .rsplit('/')
            .next()
            .and_then(|segment| segment.rsplit_once('.').map(|(_, extension)| extension))
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mcap")),
        _ => false,
    };

    let open_existing = |parsed: ViewerOpenUrl| {
        parsed.open(
            egui_ctx,
            &open_url::OpenUrlOptions {
                recording_open_behavior: RecordingOpenBehavior::OpenAndSelect,
                show_loader: true,
            },
            command_sender,
        );
    };

    if !is_explicit_remote_mcap {
        open_existing(parsed);
        return Ok(CompatibilityUrlDispatchOutcomeV1::ExistingDispatcher);
    }

    match remote_dispatch() {
        CompatibilityRemoteMcapControlV1::ExistingDispatcher => {
            open_existing(parsed);
            Ok(CompatibilityUrlDispatchOutcomeV1::ExistingDispatcher)
        }
        CompatibilityRemoteMcapControlV1::RemoteAccepted => {
            Ok(CompatibilityUrlDispatchOutcomeV1::RemoteAccepted)
        }
        CompatibilityRemoteMcapControlV1::RemoteSessionLimitReached => {
            Ok(CompatibilityUrlDispatchOutcomeV1::RemoteSessionLimitReached)
        }
    }
}

/// A `JavaScript` string or array of strings, preserving caller order.
#[derive(Clone, Debug)]
pub(crate) struct StringOrStringArray(Vec<String>);

impl StringOrStringArray {
    pub(crate) fn into_inner(self) -> Vec<String> {
        self.0
    }
}

impl From<Vec<String>> for StringOrStringArray {
    fn from(value: Vec<String>) -> Self {
        Self(value)
    }
}

impl std::ops::Deref for StringOrStringArray {
    type Target = Vec<String>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(target_arch = "wasm32")]
impl<'de> serde::Deserialize<'de> for StringOrStringArray {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use wasm_bindgen::{JsCast as _, JsValue};

        fn from_value(value: JsValue) -> Option<Vec<String>> {
            if let Some(value) = value.as_string() {
                return Some(vec![value]);
            }

            let array = value.dyn_into::<js_sys::Array>().ok()?;
            let mut out = Vec::with_capacity(array.length() as usize);
            for item in array {
                out.push(item.as_string()?);
            }
            Some(out)
        }

        let value = serde_wasm_bindgen::preserve::deserialize(deserializer)?;
        from_value(value)
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("value is not a string or array of strings"))
    }
}

/// Dispatches hidden startup URLs through the existing compatibility parser and command path.
pub(crate) fn dispatch_hidden_startup_urls(
    urls: StringOrStringArray,
    egui_ctx: &egui::Context,
    command_sender: &CommandSender,
) {
    for url in urls.into_inner() {
        match url.parse::<open_url::ViewerOpenUrl>() {
            Ok(url) => {
                url.open(
                    egui_ctx,
                    &open_url::OpenUrlOptions {
                        recording_open_behavior: RecordingOpenBehavior::OpenAndSelect,
                        show_loader: true,
                    },
                    command_sender,
                );
            }
            Err(err) => {
                re_log::warn!(?url, "Failed to open URL: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use re_data_source::LogDataSource;
    use re_log_channel::LogSource;
    use re_viewer_context::{Item, Route, SystemCommand, command_channel};

    use super::{
        CompatibilityRemoteMcapControlV1, CompatibilityUrlDispatchOutcomeV1,
        StrictHttpRemoteMcapRouteErrorV1, StrictHttpRemoteMcapRouteV1, StringOrStringArray,
        classify_strict_http_remote_mcap_route_v1, dispatch_compatibility_url_v1,
        dispatch_hidden_startup_urls,
    };

    #[test]
    fn strict_route_classifier_is_http_mcap_only_and_side_effect_free() {
        assert_eq!(
            classify_strict_http_remote_mcap_route_v1(
                "https://example.test/a.MCAP?secret=x",
                false
            ),
            Ok(StrictHttpRemoteMcapRouteV1::ExplicitMcap)
        );
        assert_eq!(
            classify_strict_http_remote_mcap_route_v1("https://example.test/no-extension", true),
            Ok(StrictHttpRemoteMcapRouteV1::ExtensionlessSniff)
        );
        assert_eq!(
            classify_strict_http_remote_mcap_route_v1("https://example.test/no-extension", false),
            Err(StrictHttpRemoteMcapRouteErrorV1::UnsupportedFormat)
        );
        assert_eq!(
            classify_strict_http_remote_mcap_route_v1("https://example.test/data.rrd", true),
            Err(StrictHttpRemoteMcapRouteErrorV1::UnsupportedFormat)
        );
        assert_eq!(
            classify_strict_http_remote_mcap_route_v1("rerun+http://127.0.0.1:9876/proxy", true),
            Err(StrictHttpRemoteMcapRouteErrorV1::UnsupportedStrictOpenRoute)
        );
        assert_eq!(
            classify_strict_http_remote_mcap_route_v1("not a URL", true),
            Err(StrictHttpRemoteMcapRouteErrorV1::InvalidUrl)
        );
    }

    fn command_debug_trace(receiver: &re_viewer_context::CommandReceiver) -> Vec<String> {
        std::iter::from_fn(|| {
            receiver
                .recv_system()
                .map(|(_, command)| format!("{command:?}"))
        })
        .collect()
    }

    #[test]
    fn compatibility_add_receiver_path_diffs_to_existing_dispatcher() {
        let cases = [
            ("https://example.test/data.mcap", true),
            ("HTTP://example.test/data.MCAP?token=opaque#fragment", true),
            ("https://example.test/data.mcap?url=opaque", true),
            ("https://example.test/data.rrd", false),
            ("rerun+http://127.0.0.1:9876/proxy", false),
            (
                "rerun://127.0.0.1:1234/dataset/abc/data.mcap?segment_id=pid",
                false,
            ),
            (
                "https://viewer.example.test/?url=https%3A%2F%2Fexample.test%2Fdata.mcap",
                false,
            ),
        ];

        for (raw_url, is_remote) in cases {
            let (sender, receiver) = command_channel();
            let mut remote_calls = 0;
            let result =
                dispatch_compatibility_url_v1(raw_url, &egui::Context::default(), &sender, || {
                    remote_calls += 1;
                    CompatibilityRemoteMcapControlV1::ExistingDispatcher
                });
            assert!(
                result.is_ok(),
                "expected existing parser to accept {raw_url}"
            );
            assert_eq!(
                remote_calls,
                usize::from(is_remote),
                "remote route for {raw_url}"
            );

            let actual_trace = command_debug_trace(&receiver);
            let (expected_sender, expected_receiver) = command_channel();
            raw_url
                .parse::<super::ViewerOpenUrl>()
                .expect("baseline parser accepts route")
                .open(
                    &egui::Context::default(),
                    &re_viewer_context::open_url::OpenUrlOptions {
                        recording_open_behavior:
                            re_log_channel::RecordingOpenBehavior::OpenAndSelect,
                        show_loader: true,
                    },
                    &expected_sender,
                );
            let expected_trace = command_debug_trace(&expected_receiver);
            assert_eq!(
                actual_trace, expected_trace,
                "dispatcher trace for {raw_url}"
            );
        }

        let (sender, receiver) = command_channel();
        let mut remote_calls = 0;
        let outcome = dispatch_compatibility_url_v1(
            "https://example.test/data.mcap",
            &egui::Context::default(),
            &sender,
            || {
                remote_calls += 1;
                CompatibilityRemoteMcapControlV1::RemoteAccepted
            },
        )
        .unwrap();
        assert_eq!(outcome, CompatibilityUrlDispatchOutcomeV1::RemoteAccepted);
        assert_eq!(remote_calls, 1);
        assert!(command_debug_trace(&receiver).is_empty());

        let (sender, receiver) = command_channel();
        let outcome = dispatch_compatibility_url_v1(
            "https://example.test/data.mcap",
            &egui::Context::default(),
            &sender,
            || CompatibilityRemoteMcapControlV1::RemoteSessionLimitReached,
        )
        .unwrap();
        assert_eq!(
            outcome,
            CompatibilityUrlDispatchOutcomeV1::RemoteSessionLimitReached
        );
        assert!(command_debug_trace(&receiver).is_empty());
    }

    #[test]
    fn compatibility_extensionless_and_malformed_inputs_have_zero_remote_side_effects() {
        for raw_url in [
            "https://example.test/no-extension",
            "not a URL",
            "https://example.test/data.mcap/child",
        ] {
            let (sender, receiver) = command_channel();
            let mut remote_calls = 0;
            let result =
                dispatch_compatibility_url_v1(raw_url, &egui::Context::default(), &sender, || {
                    remote_calls += 1;
                    CompatibilityRemoteMcapControlV1::RemoteAccepted
                });
            assert!(
                result.is_err(),
                "expected existing parser rejection for {raw_url}"
            );
            assert_eq!(remote_calls, 0);
            assert!(command_debug_trace(&receiver).is_empty());
        }
    }

    #[test]
    fn hidden_startup_urls_use_web_parser_and_continue_in_order() {
        let first = "https://example.test/first.rrd";
        let malformed_for_web = "https://example.test/extensionless";
        let last = "rerun+http://127.0.0.1:9876/proxy";
        let proxy_uri: re_uri::ProxyUri = last.parse().unwrap();
        let log_rx = re_log::add_log_msg_receiver(re_log::LevelFilter::WARN);
        re_log::setup_logging();
        let (sender, receiver) = command_channel();

        dispatch_hidden_startup_urls(
            vec![
                first.to_owned(),
                malformed_for_web.to_owned(),
                last.to_owned(),
            ]
            .into(),
            &egui::Context::default(),
            &sender,
        );

        let commands: Vec<_> =
            std::iter::from_fn(|| receiver.recv_system().map(|(_, command)| command)).collect();
        assert!(matches!(
            &commands[0],
            SystemCommand::SetRoute(Route::Loading(source))
                if **source == (LogSource::HttpStream { url: first.to_owned() })
        ));
        assert!(matches!(
            &commands[1],
            SystemCommand::LoadDataSource(LogDataSource::HttpUrl { url })
                if url.as_str() == first
        ));
        assert!(matches!(
            &commands[2],
            SystemCommand::SetRoute(Route::Loading(source))
                if **source == LogSource::MessageProxy(proxy_uri.clone())
        ));
        assert!(matches!(
            &commands[3],
            SystemCommand::LoadDataSource(LogDataSource::RedapProxy(uri)) if uri == &proxy_uri
        ));
        assert!(matches!(
            &commands[4],
            SystemCommand::SetSelection(selection)
                if selection.selection == Item::RedapServer(proxy_uri.origin.clone()).into()
        ));
        assert_eq!(commands.len(), 5);

        let warnings: Vec<_> = log_rx.try_iter().collect();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains("Failed to open URL"))
        );
    }

    #[test]
    fn string_or_string_array_preserves_input_order() {
        let urls = StringOrStringArray::from(vec!["z".to_owned(), "a".to_owned()]);
        assert_eq!(urls.into_inner(), ["z", "a"]);
    }
}
