use re_log_channel::RecordingOpenBehavior;
use re_viewer_context::{CommandSender, open_url};

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

    use super::{StringOrStringArray, dispatch_hidden_startup_urls};

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
