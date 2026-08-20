//! Startup visibility tracking for the production-disarmed remote-MCAP seam.
//!
//! The native host compiles only the pure state machine used by the tests.
//! The real DOM guard is compiled for `wasm32` because it must own a browser
//! listener without capturing the viewer [`App`](crate::App).

/// A browser visibility snapshot at one startup observation point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VisibilitySnapshot {
    Visible,
    Hidden,
}

/// Whether a remote-MCAP startup result may be published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupVisibilityOutcome {
    VisibleStartupAllowed,
    RemoteStartupPublishDenied,
}

/// Pure startup visibility state used by the wasm guard and by native tests.
///
/// The tracker intentionally does not know anything about the viewer app or
/// the remote-MCAP capability. It records the three required observations:
/// before listener installation, immediately after installation, and after
/// `WebRunner::prepare_app` succeeds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StartupVisibilityBootstrapV1 {
    before_listener: Option<VisibilitySnapshot>,
    after_listener: Option<VisibilitySnapshot>,
    final_snapshot: Option<VisibilitySnapshot>,
    current_snapshot: Option<VisibilitySnapshot>,
    observed_hidden: bool,
}

impl StartupVisibilityBootstrapV1 {
    pub(crate) fn from_before_listener(before_listener: VisibilitySnapshot) -> Self {
        let observed_hidden = before_listener == VisibilitySnapshot::Hidden;
        Self {
            before_listener: Some(before_listener),
            after_listener: None,
            final_snapshot: None,
            current_snapshot: Some(before_listener),
            observed_hidden,
        }
    }

    /// Records a `visibilitychange` event snapshot.
    ///
    /// Repeated signals do not re-open the hidden transition. A hidden snapshot
    /// is sticky for the lifetime of this bootstrap tracker.
    pub(crate) fn record_visibility_event(&mut self, snapshot: VisibilitySnapshot) {
        if self.current_snapshot != Some(snapshot) {
            self.current_snapshot = Some(snapshot);
        }
        if snapshot == VisibilitySnapshot::Hidden {
            self.observed_hidden = true;
        }
    }

    /// Reconciles the post-install read with the pre-install snapshot.
    pub(crate) fn reconcile_after_listener(&mut self, snapshot: VisibilitySnapshot) {
        self.after_listener = Some(snapshot);
        self.record_visibility_event(snapshot);
        if self.before_listener == Some(VisibilitySnapshot::Hidden) {
            self.observed_hidden = true;
        }
    }

    /// Performs the final post-prepare read and returns the publish decision.
    pub(crate) fn final_reconcile(
        &mut self,
        snapshot: VisibilitySnapshot,
    ) -> StartupVisibilityOutcome {
        self.final_snapshot = Some(snapshot);
        self.record_visibility_event(snapshot);
        if self.before_listener == Some(VisibilitySnapshot::Hidden) {
            self.observed_hidden = true;
        }
        self.outcome()
    }

    fn outcome(&self) -> StartupVisibilityOutcome {
        if self.before_listener == Some(VisibilitySnapshot::Hidden)
            || self.observed_hidden
            || self.final_snapshot == Some(VisibilitySnapshot::Hidden)
        {
            StartupVisibilityOutcome::RemoteStartupPublishDenied
        } else {
            StartupVisibilityOutcome::VisibleStartupAllowed
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::cell::RefCell;
    use std::rc::Rc;

    use wasm_bindgen::JsCast;
    use wasm_bindgen::JsValue;
    use wasm_bindgen::closure::Closure;

    use super::{StartupVisibilityBootstrapV1, StartupVisibilityOutcome, VisibilitySnapshot};

    type StartupVisibilityCallback = dyn FnMut();

    /// Owns the bounded bootstrap `visibilitychange` listener.
    ///
    /// Dropping this guard removes the listener. The closure captures only the
    /// shared startup tracker and a browser document; it never captures or
    /// calls into the viewer app.
    pub(crate) struct StartupVisibilityBootstrapGuard {
        document: web_sys::Document,
        listener: Option<Closure<StartupVisibilityCallback>>,
        tracker: Rc<RefCell<StartupVisibilityBootstrapV1>>,
    }

    impl StartupVisibilityBootstrapGuard {
        pub(crate) fn install() -> Result<Self, JsValue> {
            let document = current_document()?;
            let before_listener = read_visibility_state(&document);
            let tracker = Rc::new(RefCell::new(
                StartupVisibilityBootstrapV1::from_before_listener(before_listener),
            ));

            let listener_document = document.clone();
            let listener_tracker = Rc::clone(&tracker);
            let callback: Box<StartupVisibilityCallback> = Box::new(move || {
                let snapshot = read_visibility_state(&listener_document);
                listener_tracker
                    .borrow_mut()
                    .record_visibility_event(snapshot);
            });
            let listener = Closure::wrap(callback);
            document
                .add_event_listener_with_callback(
                    "visibilitychange",
                    listener.as_ref().unchecked_ref(),
                )
                .map_err(|err| JsValue::from(err))?;

            let after_listener = read_visibility_state(&document);
            tracker
                .borrow_mut()
                .reconcile_after_listener(after_listener);

            Ok(Self {
                document,
                listener: Some(listener),
                tracker,
            })
        }

        pub(crate) fn final_reconcile(&self) -> StartupVisibilityOutcome {
            let snapshot = read_visibility_state(&self.document);
            self.tracker.borrow_mut().final_reconcile(snapshot)
        }
    }

    impl Drop for StartupVisibilityBootstrapGuard {
        fn drop(&mut self) {
            let Some(listener) = self.listener.take() else {
                return;
            };
            if let Err(err) = self.document.remove_event_listener_with_callback(
                "visibilitychange",
                listener.as_ref().unchecked_ref(),
            ) {
                re_log::debug!("Failed to remove startup visibility listener: {err:?}");
            }
            drop(listener);
        }
    }

    fn current_document() -> Result<web_sys::Document, JsValue> {
        web_sys::window()
            .ok_or_else(|| JsValue::from_str("Failed to get window."))?
            .document()
            .ok_or_else(|| JsValue::from_str("Failed to get window.document."))
    }

    fn read_visibility_state(document: &web_sys::Document) -> VisibilitySnapshot {
        match document.visibility_state() {
            web_sys::VisibilityState::Hidden => VisibilitySnapshot::Hidden,
            _ => VisibilitySnapshot::Visible,
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::StartupVisibilityBootstrapGuard;

#[cfg(test)]
mod tests {
    use super::{StartupVisibilityBootstrapV1, StartupVisibilityOutcome, VisibilitySnapshot};

    fn tracker(
        before: VisibilitySnapshot,
        events: impl IntoIterator<Item = VisibilitySnapshot>,
        after: VisibilitySnapshot,
        final_snapshot: VisibilitySnapshot,
    ) -> StartupVisibilityBootstrapV1 {
        let mut tracker = StartupVisibilityBootstrapV1::from_before_listener(before);
        for event in events {
            tracker.record_visibility_event(event);
        }
        tracker.reconcile_after_listener(after);
        tracker.final_reconcile(final_snapshot);
        tracker
    }

    #[test]
    fn initial_hidden_denies_even_if_later_snapshots_are_visible() {
        let tracker = tracker(
            VisibilitySnapshot::Hidden,
            [VisibilitySnapshot::Visible],
            VisibilitySnapshot::Visible,
            VisibilitySnapshot::Visible,
        );

        assert_eq!(
            tracker.outcome(),
            StartupVisibilityOutcome::RemoteStartupPublishDenied
        );
        assert_eq!(tracker.before_listener, Some(VisibilitySnapshot::Hidden));
        assert_eq!(tracker.after_listener, Some(VisibilitySnapshot::Visible));
        assert_eq!(tracker.final_snapshot, Some(VisibilitySnapshot::Visible));
        assert!(tracker.observed_hidden);
    }

    #[test]
    fn bootstrap_visible_to_hidden_denies_after_final_visible_recovery() {
        let tracker = tracker(
            VisibilitySnapshot::Visible,
            [VisibilitySnapshot::Hidden],
            VisibilitySnapshot::Hidden,
            VisibilitySnapshot::Visible,
        );

        assert_eq!(
            tracker.outcome(),
            StartupVisibilityOutcome::RemoteStartupPublishDenied
        );
        assert!(tracker.observed_hidden);
        assert_eq!(tracker.final_snapshot, Some(VisibilitySnapshot::Visible));
    }

    #[test]
    fn bootstrap_hidden_to_visible_still_denies_initial_hidden() {
        let tracker = tracker(
            VisibilitySnapshot::Hidden,
            [],
            VisibilitySnapshot::Visible,
            VisibilitySnapshot::Visible,
        );

        assert_eq!(
            tracker.outcome(),
            StartupVisibilityOutcome::RemoteStartupPublishDenied
        );
    }

    #[test]
    fn bootstrap_visible_hidden_visible_denies_hidden_transition() {
        let tracker = tracker(
            VisibilitySnapshot::Visible,
            [VisibilitySnapshot::Hidden, VisibilitySnapshot::Visible],
            VisibilitySnapshot::Visible,
            VisibilitySnapshot::Visible,
        );

        assert_eq!(
            tracker.outcome(),
            StartupVisibilityOutcome::RemoteStartupPublishDenied
        );
        assert!(tracker.observed_hidden);
    }

    #[test]
    fn repeated_visible_signals_and_no_event_after_listener_allow_startup() {
        let tracker = tracker(
            VisibilitySnapshot::Visible,
            [VisibilitySnapshot::Visible, VisibilitySnapshot::Visible],
            VisibilitySnapshot::Visible,
            VisibilitySnapshot::Visible,
        );

        assert_eq!(
            tracker.outcome(),
            StartupVisibilityOutcome::VisibleStartupAllowed
        );
        assert!(!tracker.observed_hidden);
    }

    #[test]
    fn final_hidden_denies_after_visible_bootstrap() {
        let tracker = tracker(
            VisibilitySnapshot::Visible,
            [],
            VisibilitySnapshot::Visible,
            VisibilitySnapshot::Hidden,
        );

        assert_eq!(
            tracker.outcome(),
            StartupVisibilityOutcome::RemoteStartupPublishDenied
        );
        assert!(tracker.observed_hidden);
    }
}
