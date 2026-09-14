//! Exact-length Chrome `ReadableStream` BYOB body pumping.

use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PumpPhase {
    Body { written: u64 },
    EofProbe,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PumpStep {
    Copy { offset: u64, len: NonZeroU64 },
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PumpStateError {
    EarlyEof,
    Overlong,
    ZeroProgress,
    InvalidReadResult,
}

#[derive(Debug)]
struct ExactLengthPumpState {
    expected: NonZeroU64,
    phase: PumpPhase,
}

#[expect(
    unsafe_code,
    reason = "this audited fixed-layout owner is the only MSRV-compatible fallible exact allocation boundary"
)]
mod fixed_output {
    use std::alloc::{Layout, alloc_zeroed, dealloc};
    use std::fmt;
    use std::ptr::NonNull;

    #[cfg(all(test, target_arch = "wasm32"))]
    std::thread_local! {
        static COMPLETED_DEALLOCATIONS: std::cell::Cell<u64> = const {
            std::cell::Cell::new(0)
        };
    }

    #[derive(Clone, Copy)]
    pub(super) struct ExactOutputAllocator {
        allocate_zeroed: unsafe fn(Layout) -> *mut u8,
        deallocate: unsafe fn(*mut u8, Layout),
    }

    impl fmt::Debug for ExactOutputAllocator {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("ExactOutputAllocator(<opaque>)")
        }
    }

    impl ExactOutputAllocator {
        #[cfg(target_arch = "wasm32")]
        const SYSTEM: Self = Self {
            allocate_zeroed: system_allocate_zeroed,
            deallocate: system_deallocate,
        };

        #[cfg(test)]
        pub(super) const fn for_test(
            allocate_zeroed: unsafe fn(Layout) -> *mut u8,
            deallocate: unsafe fn(*mut u8, Layout),
        ) -> Self {
            Self {
                allocate_zeroed,
                deallocate,
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    unsafe fn system_allocate_zeroed(layout: Layout) -> *mut u8 {
        // SAFETY: the caller passes a checked, non-zero layout and accepts sole ownership.
        unsafe { alloc_zeroed(layout) }
    }

    #[cfg(target_arch = "wasm32")]
    unsafe fn system_deallocate(pointer: *mut u8, layout: Layout) {
        // SAFETY: `ExactOutputBuffer::drop` supplies the original pointer and exact layout once.
        unsafe { dealloc(pointer, layout) };
    }

    pub(super) struct ExactOutputBuffer {
        pointer: NonNull<u8>,
        layout: Layout,
        allocator: ExactOutputAllocator,
    }

    impl fmt::Debug for ExactOutputBuffer {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("ExactOutputBuffer")
                .field("len", &self.layout.size())
                .finish_non_exhaustive()
        }
    }

    impl ExactOutputBuffer {
        #[cfg(target_arch = "wasm32")]
        pub(super) fn try_new_zeroed(len: usize) -> Result<Self, ExactOutputAllocationError> {
            Self::try_new_zeroed_with(len, ExactOutputAllocator::SYSTEM)
        }

        pub(super) fn try_new_zeroed_with(
            len: usize,
            allocator: ExactOutputAllocator,
        ) -> Result<Self, ExactOutputAllocationError> {
            if len == 0 {
                return Err(ExactOutputAllocationError);
            }
            let layout = Layout::array::<u8>(len).map_err(|_error| ExactOutputAllocationError)?;
            // SAFETY: the allocator contract returns exclusive storage for `layout` or null.
            let pointer = NonNull::new(unsafe { (allocator.allocate_zeroed)(layout) })
                .ok_or(ExactOutputAllocationError)?;
            Ok(Self {
                pointer,
                layout,
                allocator,
            })
        }

        pub(super) fn as_slice(&self) -> &[u8] {
            // SAFETY: the owner keeps exactly `layout.size()` initialized bytes live.
            unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.layout.size()) }
        }

        pub(super) fn as_mut_slice(&mut self) -> &mut [u8] {
            // SAFETY: this non-cloneable owner has exclusive access to the complete allocation.
            unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.layout.size()) }
        }

        #[cfg(test)]
        pub(super) const fn retained_bytes(&self) -> usize {
            self.layout.size()
        }
    }

    impl Drop for ExactOutputBuffer {
        fn drop(&mut self) {
            // SAFETY: this owner deallocates its original pointer and layout exactly once.
            unsafe { (self.allocator.deallocate)(self.pointer.as_ptr(), self.layout) };
            #[cfg(all(test, target_arch = "wasm32"))]
            COMPLETED_DEALLOCATIONS.with(|count| {
                count.set(
                    count
                        .get()
                        .checked_add(1)
                        .expect("test deallocation counter does not overflow"),
                );
            });
        }
    }

    #[cfg(all(test, target_arch = "wasm32"))]
    pub(super) fn completed_deallocation_count_for_test() -> u64 {
        COMPLETED_DEALLOCATIONS.with(std::cell::Cell::get)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct ExactOutputAllocationError;

    #[cfg(test)]
    mod tests {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use super::*;

        static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
        static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

        unsafe fn null_allocate(_layout: Layout) -> *mut u8 {
            std::ptr::null_mut()
        }

        unsafe fn counting_allocate(layout: Layout) -> *mut u8 {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            // SAFETY: the test forwards the checked layout and transfers sole ownership.
            unsafe { alloc_zeroed(layout) }
        }

        unsafe fn counting_deallocate(pointer: *mut u8, layout: Layout) {
            DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            // SAFETY: the owner returns the matching pointer and exact layout once.
            unsafe { dealloc(pointer, layout) };
        }

        #[test]
        fn allocation_failure_and_exact_layout_have_no_capacity_or_owner_gap() {
            let rejecting = ExactOutputAllocator::for_test(null_allocate, counting_deallocate);
            assert!(matches!(
                ExactOutputBuffer::try_new_zeroed_with(8, rejecting),
                Err(ExactOutputAllocationError)
            ));

            let before_allocations = ALLOCATIONS.load(Ordering::Relaxed);
            let before_deallocations = DEALLOCATIONS.load(Ordering::Relaxed);
            let counting = ExactOutputAllocator::for_test(counting_allocate, counting_deallocate);
            let mut output = ExactOutputBuffer::try_new_zeroed_with(8, counting)
                .expect("test allocator returns exact storage");
            assert_eq!(output.retained_bytes(), 8);
            assert_eq!(output.as_slice(), &[0; 8]);
            output.as_mut_slice()[7] = 42;
            assert_eq!(output.as_slice()[7], 42);
            assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), before_allocations + 1);
            assert_eq!(DEALLOCATIONS.load(Ordering::Relaxed), before_deallocations);
            drop(output);
            assert_eq!(
                DEALLOCATIONS.load(Ordering::Relaxed),
                before_deallocations + 1
            );
        }
    }
}

#[cfg(target_arch = "wasm32")]
use fixed_output::ExactOutputBuffer;

impl ExactLengthPumpState {
    fn new(expected: NonZeroU64) -> Self {
        Self {
            expected,
            phase: PumpPhase::Body { written: 0 },
        }
    }

    fn requested_bytes(&self, scratch_bytes: NonZeroU64) -> Option<NonZeroU64> {
        match self.phase {
            PumpPhase::Body { written } => NonZeroU64::new(
                self.expected
                    .get()
                    .checked_sub(written)?
                    .min(scratch_bytes.get()),
            ),
            PumpPhase::EofProbe => Some(NonZeroU64::MIN),
            PumpPhase::Complete => None,
        }
    }

    fn accept(
        &mut self,
        done: bool,
        returned_bytes: u64,
        supplied_capacity: NonZeroU64,
    ) -> Result<PumpStep, PumpStateError> {
        if returned_bytes > supplied_capacity.get() {
            return Err(PumpStateError::Overlong);
        }
        match self.phase {
            PumpPhase::Body { written } => {
                if done {
                    return Err(PumpStateError::EarlyEof);
                }
                let returned_bytes =
                    NonZeroU64::new(returned_bytes).ok_or(PumpStateError::ZeroProgress)?;
                let next = written
                    .checked_add(returned_bytes.get())
                    .ok_or(PumpStateError::InvalidReadResult)?;
                if next > self.expected.get() {
                    return Err(PumpStateError::Overlong);
                }
                self.phase = if next == self.expected.get() {
                    PumpPhase::EofProbe
                } else {
                    PumpPhase::Body { written: next }
                };
                Ok(PumpStep::Copy {
                    offset: written,
                    len: returned_bytes,
                })
            }
            PumpPhase::EofProbe => {
                if done && returned_bytes == 0 {
                    self.phase = PumpPhase::Complete;
                    Ok(PumpStep::Complete)
                } else if !done && returned_bytes == 0 {
                    Err(PumpStateError::ZeroProgress)
                } else {
                    Err(PumpStateError::Overlong)
                }
            }
            PumpPhase::Complete => Err(PumpStateError::InvalidReadResult),
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::fmt;
    use std::future::{Future, poll_fn};
    use std::num::NonZeroU64;
    use std::ops::Range;
    use std::pin::Pin;
    use std::task::Poll;
    use std::time::Duration;

    use futures::pin_mut;
    use js_sys::{Array, Function, Promise, Reflect, Uint8Array};
    use wasm_bindgen::{JsCast as _, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{ExactLengthPumpState, ExactOutputBuffer, PumpStateError, PumpStep};
    use crate::remote_limits::{
        ActiveRangeBodyPumpReservation, RangeAttemptAccountingBinding,
        RangeBodyPumpReservationSpec, RangeResponseAccountingScope, ScopeAccountingError,
        WasmModuleLimitAccountingRoot, WorkUnitAccountingScope,
    };

    /// The exact requested range and fixed per-read scratch ceiling for one body pump.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ExactLengthByobPumpConfig {
        requested_range: Range<u64>,
        pump_slice_bytes: NonZeroU64,
    }

    impl ExactLengthByobPumpConfig {
        pub fn new(
            requested_range: Range<u64>,
            pump_slice_bytes: NonZeroU64,
        ) -> Result<Self, ExactLengthByobPumpError> {
            let expected_bytes = requested_range
                .end
                .checked_sub(requested_range.start)
                .and_then(NonZeroU64::new)
                .ok_or(ExactLengthByobPumpError::InvalidConfiguration)?;
            if usize::try_from(expected_bytes.get()).is_err()
                || u32::try_from(pump_slice_bytes.get()).is_err()
            {
                return Err(ExactLengthByobPumpError::InvalidConfiguration);
            }
            Ok(Self {
                requested_range,
                pump_slice_bytes,
            })
        }

        pub fn requested_range(&self) -> Range<u64> {
            self.requested_range.clone()
        }

        pub fn expected_bytes(&self) -> NonZeroU64 {
            NonZeroU64::new(self.requested_range.end - self.requested_range.start)
                .expect("validated requested range is non-empty")
        }

        pub const fn pump_slice_bytes(&self) -> NonZeroU64 {
            self.pump_slice_bytes
        }
    }

    /// A redacted, stable failure from the exact-length Chrome body pump.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ExactLengthByobPumpError {
        InvalidConfiguration,
        ResourceLimitExceeded,
        AllocationFailed,
        BrowserFetchUnavailable,
        ReadFailed,
        Timeout,
        InvalidRangeBodyLength,
        InvalidByobReadResult,
        DetachedReturnedView,
        Cancelled,
    }

    impl fmt::Display for ExactLengthByobPumpError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(match self {
                Self::InvalidConfiguration => "invalid exact-length body-pump configuration",
                Self::ResourceLimitExceeded => "remote body-pump resource limit exceeded",
                Self::AllocationFailed => "remote body-pump allocation failed",
                Self::BrowserFetchUnavailable => "browser streaming body reader unavailable",
                Self::ReadFailed => "browser streaming body read failed",
                Self::Timeout => "browser streaming body read timed out",
                Self::InvalidRangeBodyLength => "range response body has an invalid length",
                Self::InvalidByobReadResult => "browser BYOB reader returned an invalid result",
                Self::DetachedReturnedView => "browser BYOB reader returned a detached view",
                Self::Cancelled => "browser streaming body read was cancelled",
            })
        }
    }

    impl std::error::Error for ExactLengthByobPumpError {}

    /// The mandatory per-slice page/control admission hook for the body pump.
    ///
    /// Implementations park while reads are disallowed and only resolve after revalidating their
    /// captured execution identity.
    #[async_trait::async_trait(?Send)]
    pub trait ExactLengthByobPumpControl {
        async fn wait_until_read_allowed(&mut self) -> Result<(), ExactLengthByobPumpControlError>;
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ExactLengthByobPumpControlError {
        Cancelled,
        StaleExecution,
    }

    /// Exact Wasm-owned response bytes coupled to their non-cloneable accounting permit.
    pub struct ExactLengthRangeBody {
        bytes: Option<ExactOutputBuffer>,
        reservation: Option<ActiveRangeBodyPumpReservation>,
        #[cfg(test)]
        after_bytes_drop_probe: Option<Box<dyn FnOnce()>>,
    }

    /// A fixed-capacity application prefix whose unread response tail is never retained.
    pub(crate) struct BoundedPrefixBody {
        bytes: Option<ExactOutputBuffer>,
        len: usize,
        reservation: Option<ActiveRangeBodyPumpReservation>,
    }

    impl BoundedPrefixBody {
        pub(crate) fn as_slice(&self) -> &[u8] {
            &self
                .bytes
                .as_ref()
                .expect("live bounded prefix owns its bytes")
                .as_slice()[..self.len]
        }
    }

    impl fmt::Debug for BoundedPrefixBody {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("BoundedPrefixBody")
                .field("len", &self.len)
                .finish_non_exhaustive()
        }
    }

    impl Drop for BoundedPrefixBody {
        fn drop(&mut self) {
            drop(self.bytes.take());
            drop(self.reservation.take());
        }
    }

    impl fmt::Debug for ExactLengthRangeBody {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("ExactLengthRangeBody")
                .field("len", &self.len())
                .finish_non_exhaustive()
        }
    }

    impl ExactLengthRangeBody {
        pub fn as_slice(&self) -> &[u8] {
            self.bytes
                .as_ref()
                .expect("live exact-length body owns its bytes")
                .as_slice()
        }

        pub fn len(&self) -> usize {
            self.as_slice().len()
        }

        pub fn is_empty(&self) -> bool {
            self.as_slice().is_empty()
        }

        #[cfg(test)]
        pub(super) fn install_after_bytes_drop_probe(&mut self, probe: impl FnOnce() + 'static) {
            assert!(self.after_bytes_drop_probe.is_none());
            self.after_bytes_drop_probe = Some(Box::new(probe));
        }
    }

    struct PreparedPumpResources {
        // Field order is normative: bytes are released before their accounting permit.
        output: Option<ExactOutputBuffer>,
        reservation: Option<ActiveRangeBodyPumpReservation>,
    }

    impl PreparedPumpResources {
        fn output_mut(&mut self) -> &mut ExactOutputBuffer {
            self.output
                .as_mut()
                .expect("prepared body pump owns exact output")
        }

        fn into_body(mut self) -> ExactLengthRangeBody {
            ExactLengthRangeBody {
                bytes: self.output.take(),
                reservation: self.reservation.take(),
                #[cfg(test)]
                after_bytes_drop_probe: None,
            }
        }

        fn into_prefix(
            mut self,
            len: usize,
        ) -> Result<BoundedPrefixBody, ExactLengthByobPumpError> {
            if self
                .output
                .as_ref()
                .is_none_or(|output| len > output.as_slice().len())
            {
                return Err(ExactLengthByobPumpError::InvalidByobReadResult);
            }
            Ok(BoundedPrefixBody {
                bytes: self.output.take(),
                len,
                reservation: self.reservation.take(),
            })
        }
    }

    /// Non-cloneable, fully fallible pre-Fetch preparation for one exact-length body pump.
    pub(crate) struct PreparedExactLengthRangeBodyPump {
        config: ExactLengthByobPumpConfig,
        resources: PreparedPumpResources,
        scratch_constructor: Uint8ArrayConstructor,
        accounting: RangeAttemptAccountingBinding,
    }

    impl fmt::Debug for PreparedExactLengthRangeBodyPump {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter
                .debug_struct("PreparedExactLengthRangeBodyPump")
                .field("config", &self.config)
                .finish_non_exhaustive()
        }
    }

    impl PreparedExactLengthRangeBodyPump {
        pub(crate) const fn accounting_binding(&self) -> RangeAttemptAccountingBinding {
            self.accounting
        }
    }

    impl Drop for ExactLengthRangeBody {
        fn drop(&mut self) {
            // Wasm bytes must disappear before their accounting permit is refunded.
            drop(self.bytes.take());
            #[cfg(test)]
            if let Some(probe) = self.after_bytes_drop_probe.take() {
                probe();
            }
            drop(self.reservation.take());
        }
    }

    struct ReaderCleanup {
        reader: Option<web_sys::ReadableStreamByobReader>,
        abort_controller: web_sys::AbortController,
        abort_on_drop: bool,
    }

    impl ReaderCleanup {
        fn new(abort_controller: &web_sys::AbortController) -> Self {
            Self {
                reader: None,
                abort_controller: abort_controller.clone(),
                abort_on_drop: true,
            }
        }

        fn install_reader(&mut self, reader: web_sys::ReadableStreamByobReader) {
            self.reader = Some(reader);
        }

        fn reader(&self) -> &web_sys::ReadableStreamByobReader {
            self.reader
                .as_ref()
                .expect("reader is installed before the pump starts")
        }

        fn release_lock(mut self) -> Result<(), ExactLengthByobPumpError> {
            let reader = self
                .reader
                .as_ref()
                .expect("reader is installed before successful release");
            call_method0(reader.as_ref(), "releaseLock")
                .map_err(|_error| ExactLengthByobPumpError::ReadFailed)?;
            self.reader.take();
            self.abort_on_drop = false;
            Ok(())
        }
    }

    impl Drop for ReaderCleanup {
        fn drop(&mut self) {
            if let Some(reader) = self.reader.take() {
                // Cancellation is best-effort and must not create a second unbounded async owner.
                let _cancel_result = call_method0(reader.as_ref(), "cancel");
            }
            if self.abort_on_drop {
                self.abort_controller.abort();
            }
        }
    }

    struct Uint8ArrayConstructor(Function);

    impl Uint8ArrayConstructor {
        fn from_global() -> Result<Self, ExactLengthByobPumpError> {
            Reflect::get(&js_sys::global(), &JsValue::from_str("Uint8Array"))
                .map_err(|_error| ExactLengthByobPumpError::BrowserFetchUnavailable)?
                .dyn_into::<Function>()
                .map(Self)
                .map_err(|_error| ExactLengthByobPumpError::BrowserFetchUnavailable)
        }

        fn allocate(&self, len: NonZeroU64) -> Result<Uint8Array, ExactLengthByobPumpError> {
            let len = u32::try_from(len.get())
                .map_err(|_error| ExactLengthByobPumpError::InvalidConfiguration)?;
            let arguments = Array::new();
            arguments.push(&JsValue::from_f64(f64::from(len)));
            Reflect::construct(&self.0, &arguments)
                .map_err(|_error| ExactLengthByobPumpError::AllocationFailed)?
                .dyn_into::<Uint8Array>()
                .map_err(|_error| ExactLengthByobPumpError::AllocationFailed)
        }
    }

    struct ParsedRead {
        done: bool,
        view: Option<Uint8Array>,
        returned_bytes: u64,
    }

    impl ParsedRead {
        fn from_js(
            value: &JsValue,
            supplied_capacity: NonZeroU64,
        ) -> Result<Self, ExactLengthByobPumpError> {
            if !value.is_object() {
                return Err(ExactLengthByobPumpError::InvalidByobReadResult);
            }
            let done = Reflect::get(value, &JsValue::from_str("done"))
                .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?
                .as_bool()
                .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
            let returned = Reflect::get(value, &JsValue::from_str("value"))
                .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?;
            if returned.is_null() || returned.is_undefined() {
                return if done {
                    Ok(Self {
                        done,
                        view: None,
                        returned_bytes: 0,
                    })
                } else {
                    Err(ExactLengthByobPumpError::InvalidByobReadResult)
                };
            }
            let view = returned
                .dyn_into::<Uint8Array>()
                .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?;
            let returned_bytes = u64::from(view.byte_length());
            let buffer = Reflect::get(view.as_ref(), &JsValue::from_str("buffer"))
                .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?
                .dyn_into::<js_sys::ArrayBuffer>()
                .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?;
            let buffer_len = u64::from(buffer.byte_length());
            let byte_offset = u64::from(view.byte_offset());
            let end = byte_offset
                .checked_add(returned_bytes)
                .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
            if end > buffer_len || buffer_len > supplied_capacity.get() {
                return Err(ExactLengthByobPumpError::InvalidByobReadResult);
            }
            if !done && buffer_len == 0 {
                return Err(ExactLengthByobPumpError::DetachedReturnedView);
            }
            Ok(Self {
                done,
                view: Some(view),
                returned_bytes,
            })
        }
    }

    fn call_method0(receiver: &JsValue, name: &str) -> Result<JsValue, JsValue> {
        Reflect::get(receiver, &JsValue::from_str(name))?
            .dyn_into::<Function>()?
            .call0(receiver)
    }

    fn start_read(
        reader: &web_sys::ReadableStreamByobReader,
        scratch: &Uint8Array,
    ) -> Result<Promise, ExactLengthByobPumpError> {
        Reflect::get(reader.as_ref(), &JsValue::from_str("read"))
            .map_err(|_error| ExactLengthByobPumpError::ReadFailed)?
            .dyn_into::<Function>()
            .map_err(|_error| ExactLengthByobPumpError::ReadFailed)?
            .call1(reader.as_ref(), scratch.as_ref())
            .map_err(|_error| ExactLengthByobPumpError::ReadFailed)?
            .dyn_into::<Promise>()
            .map_err(|_error| ExactLengthByobPumpError::ReadFailed)
    }

    async fn await_read_or_timeout<T>(
        promise: Promise,
        mut timeout: Pin<&mut T>,
    ) -> Result<JsValue, ExactLengthByobPumpError>
    where
        T: Future<Output = ()>,
    {
        let read = JsFuture::from(promise);
        pin_mut!(read);
        poll_fn(|context| {
            if timeout.as_mut().poll(context).is_ready() {
                return Poll::Ready(Err(ExactLengthByobPumpError::Timeout));
            }
            match read.as_mut().poll(context) {
                Poll::Ready(Ok(value)) => Poll::Ready(Ok(value)),
                Poll::Ready(Err(_error)) => Poll::Ready(Err(ExactLengthByobPumpError::ReadFailed)),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }

    async fn await_read_admission<T, C>(
        control: &mut C,
        mut timeout: Pin<&mut T>,
    ) -> Result<(), ExactLengthByobPumpError>
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        let admission = control.wait_until_read_allowed();
        pin_mut!(admission);
        poll_fn(|context| {
            if timeout.as_mut().poll(context).is_ready() {
                return Poll::Ready(Err(ExactLengthByobPumpError::Timeout));
            }
            match admission.as_mut().poll(context) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                Poll::Ready(Err(
                    ExactLengthByobPumpControlError::Cancelled
                    | ExactLengthByobPumpControlError::StaleExecution,
                )) => Poll::Ready(Err(ExactLengthByobPumpError::Cancelled)),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }

    fn map_accounting_error(error: ScopeAccountingError) -> ExactLengthByobPumpError {
        if error == ScopeAccountingError::AllocationFailed {
            ExactLengthByobPumpError::AllocationFailed
        } else {
            ExactLengthByobPumpError::ResourceLimitExceeded
        }
    }

    fn map_state_error(error: PumpStateError) -> ExactLengthByobPumpError {
        match error {
            PumpStateError::EarlyEof | PumpStateError::Overlong | PumpStateError::ZeroProgress => {
                ExactLengthByobPumpError::InvalidRangeBodyLength
            }
            PumpStateError::InvalidReadResult => ExactLengthByobPumpError::InvalidByobReadResult,
        }
    }

    /// Reserves accounting, allocates exact output, and resolves browser constructors before a
    /// physical Fetch-attempt burn can commit.
    pub(crate) fn prepare_exact_range_body_pump(
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        config: ExactLengthByobPumpConfig,
    ) -> Result<PreparedExactLengthRangeBodyPump, ExactLengthByobPumpError> {
        let expected_bytes = config.expected_bytes();
        let expected_len = usize::try_from(expected_bytes.get())
            .map_err(|_error| ExactLengthByobPumpError::InvalidConfiguration)?;
        let accounting = range_scope
            .range_attempt_accounting_binding(work_scope)
            .map_err(map_accounting_error)?;
        let reservation = range_scope
            .prepare_body_pump_reservation(
                root,
                work_scope,
                RangeBodyPumpReservationSpec {
                    output_bytes: expected_bytes,
                    scratch_bytes: config.pump_slice_bytes(),
                },
            )
            .and_then(|prepared| prepared.commit())
            .map_err(map_accounting_error)?;
        let output = ExactOutputBuffer::try_new_zeroed(expected_len)
            .map_err(|_allocation_error| ExactLengthByobPumpError::AllocationFailed)?;
        let scratch_constructor = Uint8ArrayConstructor::from_global()?;
        Ok(PreparedExactLengthRangeBodyPump {
            config,
            resources: PreparedPumpResources {
                output: Some(output),
                reservation: Some(reservation),
            },
            scratch_constructor,
            accounting,
        })
    }

    /// Reads one already-validated `206` response body into an exact Wasm-owned buffer.
    ///
    /// `timeout` must use the caller's active-visible deadline rather than raw hidden-page wall
    /// time.
    pub async fn read_exact_range_body<T, C>(
        response: &web_sys::Response,
        abort_controller: &web_sys::AbortController,
        root: &WasmModuleLimitAccountingRoot,
        range_scope: &RangeResponseAccountingScope,
        work_scope: &WorkUnitAccountingScope,
        config: ExactLengthByobPumpConfig,
        control: &mut C,
        timeout: T,
    ) -> Result<ExactLengthRangeBody, ExactLengthByobPumpError>
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        let prepared = prepare_exact_range_body_pump(root, range_scope, work_scope, config);
        let prepared = match prepared {
            Ok(reservation) => reservation,
            Err(error) => {
                abort_controller.abort();
                return Err(error);
            }
        };

        read_exact_range_body_prepared(response, abort_controller, prepared, control, timeout).await
    }

    /// Pumps a response using a capability whose accounting, exact output allocation, and browser
    /// constructor lookup all completed before Fetch started.
    pub(crate) async fn read_exact_range_body_prepared<T, C>(
        response: &web_sys::Response,
        abort_controller: &web_sys::AbortController,
        prepared: PreparedExactLengthRangeBodyPump,
        control: &mut C,
        timeout: T,
    ) -> Result<ExactLengthRangeBody, ExactLengthByobPumpError>
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        read_exact_range_body_prepared_with_completion(
            response,
            abort_controller,
            prepared,
            control,
            timeout,
            |_body| ExactLengthBodyCompletion::ReleaseReader,
        )
        .await
    }

    /// Reader disposition selected only after exact bytes and immediate EOF have been proven.
    pub(crate) enum ExactLengthBodyCompletion {
        ReleaseReader,
        CancelAndAbort,
    }

    /// Exact-length pump variant whose caller can cancel a now-proven non-matching format before
    /// the reader capability is released.
    pub(crate) async fn read_exact_range_body_prepared_with_completion<T, C>(
        response: &web_sys::Response,
        abort_controller: &web_sys::AbortController,
        prepared: PreparedExactLengthRangeBodyPump,
        control: &mut C,
        timeout: T,
        completion: impl FnOnce(&ExactLengthRangeBody) -> ExactLengthBodyCompletion,
    ) -> Result<ExactLengthRangeBody, ExactLengthByobPumpError>
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        let PreparedExactLengthRangeBodyPump {
            config,
            mut resources,
            scratch_constructor,
            accounting: _,
        } = prepared;
        let expected_bytes = config.expected_bytes();

        let mut cleanup = ReaderCleanup::new(abort_controller);
        let stream = response
            .body()
            .ok_or(ExactLengthByobPumpError::BrowserFetchUnavailable)?;
        let reader = web_sys::ReadableStreamByobReader::new(&stream)
            .map_err(|_error| ExactLengthByobPumpError::BrowserFetchUnavailable)?;
        cleanup.install_reader(reader);
        let mut state = ExactLengthPumpState::new(expected_bytes);
        pin_mut!(timeout);

        loop {
            await_read_admission(control, timeout.as_mut()).await?;
            let requested = state
                .requested_bytes(config.pump_slice_bytes())
                .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
            let scratch = scratch_constructor.allocate(requested)?;
            let read = start_read(cleanup.reader(), &scratch)?;
            // Chrome may detach `scratch` while fulfilling the read.
            // Never inspect or reuse it after this await.
            let result = await_read_or_timeout(read, timeout.as_mut()).await?;
            drop(scratch);
            let parsed = ParsedRead::from_js(&result, requested)?;
            let step = state
                .accept(parsed.done, parsed.returned_bytes, requested)
                .map_err(map_state_error)?;
            match step {
                PumpStep::Copy { offset, len } => {
                    let offset = usize::try_from(offset)
                        .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?;
                    let len = usize::try_from(len.get())
                        .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?;
                    let end = offset
                        .checked_add(len)
                        .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
                    let destination = resources
                        .output_mut()
                        .as_mut_slice()
                        .get_mut(offset..end)
                        .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
                    parsed
                        .view
                        .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?
                        .copy_to(destination);
                    re_async::sleep(Duration::ZERO).await;
                }
                PumpStep::Complete => {
                    let body = resources.into_body();
                    match completion(&body) {
                        ExactLengthBodyCompletion::ReleaseReader => cleanup.release_lock()?,
                        ExactLengthBodyCompletion::CancelAndAbort => drop(cleanup),
                    }
                    return Ok(body);
                }
            }
        }
    }

    /// Reads at most the prepared fixed capacity and then always cancels the reader and aborts the
    /// request.
    ///
    /// Unlike the exact Range pump this accepts early EOF and deliberately performs no trailing
    /// EOF probe. It is reserved for the strict extensionless `200 OK` format-sniff exception.
    pub(crate) async fn read_bounded_prefix_body_prepared<T, C>(
        response: &web_sys::Response,
        abort_controller: &web_sys::AbortController,
        prepared: PreparedExactLengthRangeBodyPump,
        control: &mut C,
        timeout: T,
    ) -> Result<BoundedPrefixBody, ExactLengthByobPumpError>
    where
        T: Future<Output = ()>,
        C: ExactLengthByobPumpControl,
    {
        let PreparedExactLengthRangeBodyPump {
            config,
            mut resources,
            scratch_constructor,
            accounting: _,
        } = prepared;
        let capacity = config.expected_bytes().get();
        let mut cleanup = ReaderCleanup::new(abort_controller);
        let stream = response
            .body()
            .ok_or(ExactLengthByobPumpError::BrowserFetchUnavailable)?;
        let reader = web_sys::ReadableStreamByobReader::new(&stream)
            .map_err(|_error| ExactLengthByobPumpError::BrowserFetchUnavailable)?;
        cleanup.install_reader(reader);
        let mut written = 0u64;
        pin_mut!(timeout);

        loop {
            let remaining = capacity
                .checked_sub(written)
                .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
            if remaining == 0 {
                return resources.into_prefix(
                    usize::try_from(written)
                        .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?,
                );
            }
            await_read_admission(control, timeout.as_mut()).await?;
            let requested = NonZeroU64::new(remaining.min(config.pump_slice_bytes().get()))
                .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
            let scratch = scratch_constructor.allocate(requested)?;
            let read = start_read(cleanup.reader(), &scratch)?;
            let result = await_read_or_timeout(read, timeout.as_mut()).await?;
            drop(scratch);
            let parsed = ParsedRead::from_js(&result, requested)?;
            if parsed.returned_bytes == 0 {
                if parsed.done {
                    return resources.into_prefix(
                        usize::try_from(written)
                            .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?,
                    );
                }
                return Err(ExactLengthByobPumpError::InvalidRangeBodyLength);
            }
            let offset = usize::try_from(written)
                .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?;
            let len = usize::try_from(parsed.returned_bytes)
                .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?;
            let end = offset
                .checked_add(len)
                .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
            parsed
                .view
                .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?
                .copy_to(
                    resources
                        .output_mut()
                        .as_mut_slice()
                        .get_mut(offset..end)
                        .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?,
                );
            written = written
                .checked_add(parsed.returned_bytes)
                .ok_or(ExactLengthByobPumpError::InvalidByobReadResult)?;
            if parsed.done || written == capacity {
                return resources.into_prefix(
                    usize::try_from(written)
                        .map_err(|_error| ExactLengthByobPumpError::InvalidByobReadResult)?,
                );
            }
            re_async::sleep(Duration::ZERO).await;
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::*;

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use futures::future;
    use wasm_bindgen::{JsCast as _, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::{ExactLengthByobPumpConfig, ExactLengthByobPumpError, read_exact_range_body};

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
        export function make_mcap_byob_response(totalBytes, pending, infinite) {
            let remaining = totalBytes;
            let nextByte = { value: 0 };
            const stream = new ReadableStream({
                type: "bytes",
                pull(controller) {
                    if (pending) {
                        return;
                    }
                    if (!infinite && remaining === 0) {
                        controller.close();
                        return;
                    }
                    const request = controller.byobRequest;
                    if (!request) {
                        throw new Error("missing BYOB request");
                    }
                    const count = infinite ? 1 : Math.min(request.view.byteLength, remaining);
                    for (let index = 0; index < count; index += 1) {
                        request.view[index] = nextByte.value;
                        nextByte.value = (nextByte.value + 1) & 255;
                    }
                    if (!infinite) {
                        remaining -= count;
                    }
                    request.respond(count);
                    if (!infinite && remaining === 0) {
                        controller.close();
                    }
                },
            });
            return new Response(stream);
        }

        export function make_mcap_non_byob_response() {
            const stream = new ReadableStream({
                start(controller) {
                    controller.enqueue("not a byte stream");
                    controller.close();
                },
            });
            return new Response(stream);
        }

        export function patch_next_mcap_byob_read(mode) {
            const prototype = ReadableStreamBYOBReader.prototype;
            const original = prototype.read;
            prototype.read = function(view) {
                prototype.read = original;
                if (mode === "zero") {
                    return Promise.resolve({
                        done: false,
                        value: new Uint8Array(new ArrayBuffer(1), 0, 0),
                    });
                }
                if (mode === "detached") {
                    const buffer = new ArrayBuffer(1);
                    const returned = new Uint8Array(buffer);
                    structuredClone(buffer, { transfer: [buffer] });
                    return Promise.resolve({ done: false, value: returned });
                }
                if (mode === "oversized_backing") {
                    return Promise.resolve({
                        done: false,
                        value: new Uint8Array(new ArrayBuffer(16), 0, 1),
                    });
                }
                return Promise.resolve({ done: false, value: {} });
            };
        }

        let originalCountedRead;
        let countedReads = 0;
        export function begin_counting_mcap_byob_reads() {
            const prototype = ReadableStreamBYOBReader.prototype;
            originalCountedRead = prototype.read;
            countedReads = 0;
            prototype.read = function(view) {
                countedReads += 1;
                return originalCountedRead.call(this, view);
            };
        }

        export function finish_counting_mcap_byob_reads() {
            if (originalCountedRead !== undefined) {
                ReadableStreamBYOBReader.prototype.read = originalCountedRead;
                originalCountedRead = undefined;
            }
            return countedReads;
        }
    "#)]
    extern "C" {
        fn make_mcap_byob_response(total_bytes: u32, pending: bool, infinite: bool) -> JsValue;
        fn make_mcap_non_byob_response() -> JsValue;
        fn patch_next_mcap_byob_read(mode: &str);
        fn begin_counting_mcap_byob_reads();
        fn finish_counting_mcap_byob_reads() -> u32;
    }

    #[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
        const PRODUCTION_PUMP_FIXTURE_PROTOCOL = "mcap-range-fixture-v1";
        let productionPumpInstrumentation;

        function productionPumpFixturePort() {
            const rawPort = new URLSearchParams(window.location.search).get("mcap_fixture_page_port");
            if (rawPort === null || !/^\d+$/.test(rawPort)) {
                throw new Error("missing production pump fixture port");
            }
            const port = Number(rawPort);
            if (!Number.isSafeInteger(port) || port <= 0 || port > 65_535) {
                throw new Error("invalid production pump fixture port");
            }
            return port;
        }

        function productionPumpBody(kind) {
            const chunk = (length, delay_ms = 0) => ({ length, delay_ms, wait_for_gate: null });
            switch (kind) {
                case "exact":
                    return { type: "finite", chunks: [chunk(1, 2), chunk(1, 2), chunk(1, 2), chunk(1, 2)] };
                case "heartbeat":
                    return { type: "finite", chunks: [chunk(64)] };
                case "early_eof":
                    return { type: "finite", chunks: [chunk(2)] };
                case "overlong":
                    return { type: "finite", chunks: [chunk(5)] };
                case "infinite":
                    return { type: "infinite", chunk: chunk(1, 1) };
                case "stall":
                    return { type: "stall_after", chunks: [] };
                case "release_failure":
                    return { type: "finite", chunks: [chunk(4)] };
                default:
                    throw new Error("unknown production pump scenario kind");
            }
        }

        export function is_chrome_production_pump_runtime() {
            return /(?:Chrome|Chromium)/.test(navigator.userAgent);
        }

        export async function register_production_pump_scenario(kind) {
            const bootstrapResponse = await fetch(
                `http://127.0.0.1:${productionPumpFixturePort()}/__mcap_range_fixture/v1/bootstrap`,
                { cache: "no-store" },
            );
            if (!bootstrapResponse.ok) {
                throw new Error("production pump fixture bootstrap failed");
            }
            const bootstrap = await bootstrapResponse.json();
            if (bootstrap.protocol !== PRODUCTION_PUMP_FIXTURE_PROTOCOL) {
                throw new Error("production pump fixture protocol mismatch");
            }
            const spec = {
                object_length: 64,
                object_seed: 10,
                request: {
                    range: { mode: "any" },
                    if_match: { mode: "any" },
                    head: "mirror_get",
                },
                status: 206,
                content_range: { type: "fixed", start: 0, end: 63, total: 64 },
                content_length: { type: "body_length" },
                content_encoding: "omit",
                etag: { type: "strong", value: "v1" },
                cors: {
                    allow_origin: "any",
                    preflight: "allow_required_headers",
                    expose: "required_range_headers",
                },
                redirect: "none",
                csp_connect: "self_and_object_origin",
                body: productionPumpBody(kind),
                service_worker: { type: "disabled" },
                response_revisions: [],
            };
            const controlBase = `${bootstrap.page_origin}${bootstrap.control_root}/control/scenarios`;
            const response = await fetch(controlBase, {
                method: "POST",
                cache: "no-store",
                headers: { "content-type": "application/json" },
                body: JSON.stringify(spec),
            });
            if (!response.ok) {
                throw new Error("production pump scenario registration failed");
            }
            const descriptor = await response.json();
            return {
                responseUrl: descriptor.cross_origin_object_url,
                cleanupUrl: `${controlBase}/${descriptor.id}`,
            };
        }

        export async function fetch_production_pump_response(url, signal) {
            const response = await fetch(url, { cache: "no-store", signal });
            if (response.status !== 206) {
                throw new Error("production pump fixture returned the wrong status");
            }
            return response;
        }

        export async function delete_production_pump_scenario(url) {
            await fetch(url, { method: "DELETE", cache: "no-store" });
        }

        export async function wait_for_production_pump_body_cancellation(url) {
            for (let attempt = 0; attempt < 80; attempt += 1) {
                const response = await fetch(url, { cache: "no-store" });
                if (!response.ok) {
                    throw new Error("production pump event snapshot failed");
                }
                const snapshot = await response.json();
                if (snapshot.events.some((event) => event.type === "body_cancelled")) {
                    return true;
                }
                await new Promise((resolve) => window.setTimeout(resolve, 25));
            }
            return false;
        }

        export function begin_production_pump_instrumentation() {
            if (productionPumpInstrumentation !== undefined) {
                throw new Error("production pump instrumentation is already active");
            }
            const responsePrototype = Response.prototype;
            const readerPrototype = ReadableStreamBYOBReader.prototype;
            const hadOwnSetTimeout = Object.prototype.hasOwnProperty.call(window, "setTimeout");
            const ownSetTimeoutDescriptor = Object.getOwnPropertyDescriptor(window, "setTimeout");
            const originalSetTimeout = window.setTimeout;
            const stats = {
                arrayBufferCalls: 0,
                reads: 0,
                detachedInputViews: 0,
                releaseLockCalls: 0,
                cancelCalls: 0,
                heartbeat: 0,
                failNextRelease: false,
                pumpActive: false,
                activeZeroTimeoutCalls: 0,
            };
            const originals = {
                arrayBuffer: responsePrototype.arrayBuffer,
                read: readerPrototype.read,
                releaseLock: readerPrototype.releaseLock,
                cancel: readerPrototype.cancel,
            };
            responsePrototype.arrayBuffer = function(...args) {
                stats.arrayBufferCalls += 1;
                return originals.arrayBuffer.apply(this, args);
            };
            readerPrototype.read = function(view) {
                stats.reads += 1;
                const inputBuffer = view.buffer;
                return originals.read.call(this, view).then(
                    (result) => {
                        if (inputBuffer.byteLength === 0) {
                            stats.detachedInputViews += 1;
                        }
                        return result;
                    },
                    (error) => {
                        if (inputBuffer.byteLength === 0) {
                            stats.detachedInputViews += 1;
                        }
                        throw error;
                    },
                );
            };
            readerPrototype.releaseLock = function(...args) {
                stats.releaseLockCalls += 1;
                if (stats.failNextRelease) {
                    stats.failNextRelease = false;
                    throw new Error("injected releaseLock failure");
                }
                return originals.releaseLock.apply(this, args);
            };
            readerPrototype.cancel = function(...args) {
                stats.cancelCalls += 1;
                return originals.cancel.apply(this, args);
            };
            Object.defineProperty(window, "setTimeout", {
                configurable: true,
                writable: true,
                value(callback, delay, ...args) {
                    if (stats.pumpActive && Number(delay) === 0) {
                        stats.activeZeroTimeoutCalls += 1;
                    }
                    return originalSetTimeout.call(this, callback, delay, ...args);
                },
            });
            const heartbeatTimer = window.setInterval(() => {
                stats.heartbeat += 1;
            }, 0);
            productionPumpInstrumentation = {
                responsePrototype,
                readerPrototype,
                originals,
                stats,
                heartbeatTimer,
                hadOwnSetTimeout,
                ownSetTimeoutDescriptor,
            };
        }

        export function begin_production_pump_active_window() {
            if (productionPumpInstrumentation === undefined) {
                throw new Error("production pump instrumentation is inactive");
            }
            const stats = productionPumpInstrumentation.stats;
            if (stats.pumpActive) {
                throw new Error("production pump active window is already open");
            }
            stats.activeZeroTimeoutCalls = 0;
            stats.pumpActive = true;
        }

        export function end_production_pump_active_window() {
            if (productionPumpInstrumentation === undefined) {
                throw new Error("production pump instrumentation is inactive");
            }
            const stats = productionPumpInstrumentation.stats;
            if (!stats.pumpActive) {
                throw new Error("production pump active window is not open");
            }
            stats.pumpActive = false;
            return stats.activeZeroTimeoutCalls;
        }

        export function fail_next_production_pump_release() {
            if (productionPumpInstrumentation === undefined) {
                throw new Error("production pump instrumentation is inactive");
            }
            productionPumpInstrumentation.stats.failNextRelease = true;
        }

        export function production_pump_instrumentation_snapshot() {
            if (productionPumpInstrumentation === undefined) {
                throw new Error("production pump instrumentation is inactive");
            }
            const stats = productionPumpInstrumentation.stats;
            return {
                arrayBufferCalls: stats.arrayBufferCalls,
                reads: stats.reads,
                detachedInputViews: stats.detachedInputViews,
                releaseLockCalls: stats.releaseLockCalls,
                cancelCalls: stats.cancelCalls,
                heartbeat: stats.heartbeat,
            };
        }

        export function reset_production_pump_test_harness() {
            // A test that aborted under `panic = abort` never ran its guard cleanup, so its patches
            // stay installed and would break the next pumping test. The guard itself stays strict.
            finish_production_pump_instrumentation();
        }

        export function finish_production_pump_instrumentation() {
            if (productionPumpInstrumentation === undefined) {
                return;
            }
            const instrumentation = productionPumpInstrumentation;
            productionPumpInstrumentation = undefined;
            window.clearInterval(instrumentation.heartbeatTimer);
            instrumentation.responsePrototype.arrayBuffer = instrumentation.originals.arrayBuffer;
            instrumentation.readerPrototype.read = instrumentation.originals.read;
            instrumentation.readerPrototype.releaseLock = instrumentation.originals.releaseLock;
            instrumentation.readerPrototype.cancel = instrumentation.originals.cancel;
            if (instrumentation.hadOwnSetTimeout) {
                Object.defineProperty(
                    window,
                    "setTimeout",
                    instrumentation.ownSetTimeoutDescriptor,
                );
            } else {
                delete window.setTimeout;
            }
        }
    "#)]
    extern "C" {
        fn is_chrome_production_pump_runtime() -> bool;
        fn register_production_pump_scenario(kind: &str) -> js_sys::Promise;
        fn fetch_production_pump_response(
            url: &str,
            signal: &web_sys::AbortSignal,
        ) -> js_sys::Promise;
        fn delete_production_pump_scenario(url: &str) -> js_sys::Promise;
        fn wait_for_production_pump_body_cancellation(url: &str) -> js_sys::Promise;
        fn begin_production_pump_instrumentation();
        fn begin_production_pump_active_window();
        fn end_production_pump_active_window() -> u32;
        fn fail_next_production_pump_release();
        fn production_pump_instrumentation_snapshot() -> JsValue;
        fn finish_production_pump_instrumentation();
        /// Restores the observation patches this harness owns after an aborted test skipped cleanup.
        fn reset_production_pump_test_harness();
    }

    struct ProductionPumpInstrumentationGuard;

    impl ProductionPumpInstrumentationGuard {
        fn start() -> Self {
            begin_production_pump_instrumentation();
            Self
        }

        fn stat(field: &str) -> u64 {
            let snapshot = production_pump_instrumentation_snapshot();
            js_sys::Reflect::get(&snapshot, &JsValue::from_str(field))
                .expect("instrumentation snapshot has fixed fields")
                .as_f64()
                .and_then(|value| {
                    (value.is_finite() && value >= 0.0 && value.fract() == 0.0)
                        .then_some(value as u64)
                })
                .expect("instrumentation counters are non-negative integers")
        }
    }

    impl Drop for ProductionPumpInstrumentationGuard {
        fn drop(&mut self) {
            finish_production_pump_instrumentation();
        }
    }

    struct ControlledProductionPumpScenario {
        response_url: String,
        cleanup_url: String,
    }

    impl ControlledProductionPumpScenario {
        async fn register(kind: &str) -> Self {
            let descriptor = JsFuture::from(register_production_pump_scenario(kind))
                .await
                .expect("controlled production-pump scenario registers");
            Self {
                response_url: js_string_field(&descriptor, "responseUrl"),
                cleanup_url: js_string_field(&descriptor, "cleanupUrl"),
            }
        }

        async fn fetch(&self, controller: &web_sys::AbortController) -> web_sys::Response {
            JsFuture::from(fetch_production_pump_response(
                &self.response_url,
                &controller.signal(),
            ))
            .await
            .expect("controlled production-pump Fetch succeeds")
            .unchecked_into()
        }

        async fn observed_body_cancellation(&self) -> bool {
            JsFuture::from(wait_for_production_pump_body_cancellation(
                &self.cleanup_url,
            ))
            .await
            .expect("controlled fixture event snapshot succeeds")
            .as_bool()
            .expect("cancellation observation is boolean")
        }

        async fn remove(self) {
            JsFuture::from(delete_production_pump_scenario(&self.cleanup_url))
                .await
                .expect("controlled production-pump scenario is removed");
        }
    }

    fn js_string_field(object: &JsValue, field: &str) -> String {
        js_sys::Reflect::get(object, &JsValue::from_str(field))
            .expect("fixture descriptor has fixed fields")
            .as_string()
            .expect("fixture descriptor field is a string")
    }

    fn chrome_test_is_enabled() -> bool {
        is_chrome_production_pump_runtime()
    }

    struct AlwaysAllowed;

    #[async_trait::async_trait(?Send)]
    impl super::ExactLengthByobPumpControl for AlwaysAllowed {
        async fn wait_until_read_allowed(
            &mut self,
        ) -> Result<(), super::ExactLengthByobPumpControlError> {
            Ok(())
        }
    }

    struct AllowOnceThenCancel {
        admissions: u32,
    }

    #[async_trait::async_trait(?Send)]
    impl super::ExactLengthByobPumpControl for AllowOnceThenCancel {
        async fn wait_until_read_allowed(
            &mut self,
        ) -> Result<(), super::ExactLengthByobPumpControlError> {
            self.admissions += 1;
            if self.admissions == 1 {
                Ok(())
            } else {
                Err(super::ExactLengthByobPumpControlError::Cancelled)
            }
        }
    }

    fn scopes() -> (
        crate::remote_limits::WasmModuleLimitAccountingRoot,
        crate::remote_limits::RangeResponseAccountingScope,
        crate::remote_limits::WorkUnitAccountingScope,
    ) {
        let root = crate::remote_limits::tests::transport_test_profile_with(&[])
            .start_accounting_root()
            .expect("transport test profile starts");
        let viewer = root
            .create_viewer_scope()
            .expect("viewer scope is available");
        let source = viewer
            .create_source_scope()
            .expect("source scope is available");
        let session = source
            .create_session_scope()
            .expect("session scope is available");
        let range = session
            .create_range_response_scope()
            .expect("range scope is available");
        let work = range
            .create_work_unit_scope()
            .expect("work scope is available");
        (root, range, work)
    }

    async fn pump(
        response: web_sys::Response,
        expected: u64,
        timeout: impl Future<Output = ()>,
    ) -> (
        Result<super::ExactLengthRangeBody, ExactLengthByobPumpError>,
        web_sys::AbortController,
    ) {
        let mut control = AlwaysAllowed;
        pump_with_control(response, expected, &mut control, timeout).await
    }

    async fn pump_with_control(
        response: web_sys::Response,
        expected: u64,
        control: &mut impl super::ExactLengthByobPumpControl,
        timeout: impl Future<Output = ()>,
    ) -> (
        Result<super::ExactLengthRangeBody, ExactLengthByobPumpError>,
        web_sys::AbortController,
    ) {
        let (root, range, work) = scopes();
        let controller = web_sys::AbortController::new().expect("browser exposes AbortController");
        let config = ExactLengthByobPumpConfig::new(
            0..expected,
            NonZeroU64::new(2).expect("test scratch is non-zero"),
        )
        .expect("test range is valid");
        let result = read_exact_range_body(
            &response,
            &controller,
            &root,
            &range,
            &work,
            config,
            control,
            timeout,
        )
        .await;
        (result, controller)
    }

    async fn pump_controlled_response<T>(
        response: &web_sys::Response,
        controller: &web_sys::AbortController,
        root: &crate::remote_limits::WasmModuleLimitAccountingRoot,
        range: &crate::remote_limits::RangeResponseAccountingScope,
        work: &crate::remote_limits::WorkUnitAccountingScope,
        expected: u64,
        scratch: u64,
        timeout: T,
    ) -> Result<super::ExactLengthRangeBody, ExactLengthByobPumpError>
    where
        T: Future<Output = ()>,
    {
        let config = ExactLengthByobPumpConfig::new(
            0..expected,
            NonZeroU64::new(scratch).expect("controlled scratch is non-zero"),
        )
        .expect("controlled range is valid");
        let mut control = AlwaysAllowed;
        read_exact_range_body(
            response,
            controller,
            root,
            range,
            work,
            config,
            &mut control,
            timeout,
        )
        .await
    }

    async fn assert_controlled_failure<T>(
        kind: &str,
        expected_error: ExactLengthByobPumpError,
        timeout: T,
        expect_server_cancellation: bool,
        instrumentation: &ProductionPumpInstrumentationGuard,
        root: &crate::remote_limits::WasmModuleLimitAccountingRoot,
        range: &crate::remote_limits::RangeResponseAccountingScope,
        work: &crate::remote_limits::WorkUnitAccountingScope,
        baseline: crate::remote_limits::AccountingScalarSnapshot,
    ) where
        T: Future<Output = ()>,
    {
        let scenario = ControlledProductionPumpScenario::register(kind).await;
        let controller = web_sys::AbortController::new().expect("browser exposes AbortController");
        let response = scenario.fetch(&controller).await;
        let _instrumentation_lifetime = instrumentation;
        let cancels_before = ProductionPumpInstrumentationGuard::stat("cancelCalls");
        let result =
            pump_controlled_response(&response, &controller, root, range, work, 4, 2, timeout)
                .await;
        assert!(matches!(result, Err(error) if error == expected_error));
        assert!(controller.signal().aborted());
        assert!(ProductionPumpInstrumentationGuard::stat("cancelCalls") > cancels_before);
        assert_eq!(root.accounting_scalar_snapshot(), baseline);
        if expect_server_cancellation {
            assert!(
                scenario.observed_body_cancellation().await,
                "controlled server did not observe production-pump cancellation for {kind}"
            );
        }
        scenario.remove().await;
    }

    use std::future::Future;
    use std::num::NonZeroU64;
    use std::time::Duration;

    #[wasm_bindgen_test]
    async fn exact_body_succeeds_without_aborting() {
        if !chrome_test_is_enabled() {
            return;
        }
        let response = make_mcap_byob_response(4, false, false).unchecked_into();
        let (result, controller) = pump(response, 4, future::pending()).await;
        let body = result.expect("exact body succeeds");
        assert_eq!(body.as_slice(), &[0, 1, 2, 3]);
        assert!(!controller.signal().aborted());
    }

    #[wasm_bindgen_test]
    async fn early_eof_overlong_and_infinite_bodies_converge() {
        if !chrome_test_is_enabled() {
            return;
        }
        for (available, expected, infinite) in [(2, 4, false), (3, 2, false), (0, 2, true)] {
            let response = make_mcap_byob_response(available, false, infinite).unchecked_into();
            let (result, controller) = pump(response, expected, future::pending()).await;
            assert!(matches!(
                result,
                Err(ExactLengthByobPumpError::InvalidRangeBodyLength)
            ));
            assert!(controller.signal().aborted());
        }
    }

    #[wasm_bindgen_test]
    async fn timeout_missing_body_and_non_byob_stream_converge() {
        if !chrome_test_is_enabled() {
            return;
        }
        let response = make_mcap_byob_response(4, true, false).unchecked_into();
        begin_counting_mcap_byob_reads();
        let (result, controller) = pump(response, 4, future::ready(())).await;
        assert_eq!(finish_counting_mcap_byob_reads(), 0);
        assert!(matches!(result, Err(ExactLengthByobPumpError::Timeout)));
        assert!(controller.signal().aborted());

        let response = web_sys::Response::new().expect("empty response is constructible");
        let (result, controller) = pump(response, 4, future::pending()).await;
        assert!(matches!(
            result,
            Err(ExactLengthByobPumpError::BrowserFetchUnavailable)
        ));
        assert!(controller.signal().aborted());

        let response = make_mcap_non_byob_response().unchecked_into();
        let (result, controller) = pump(response, 4, future::pending()).await;
        assert!(matches!(
            result,
            Err(ExactLengthByobPumpError::BrowserFetchUnavailable)
        ));
        assert!(controller.signal().aborted());
    }

    #[wasm_bindgen_test]
    async fn every_post_yield_read_requires_fresh_control_admission() {
        if !chrome_test_is_enabled() {
            return;
        }
        let response = make_mcap_byob_response(4, false, false).unchecked_into();
        let mut control = AllowOnceThenCancel { admissions: 0 };
        begin_counting_mcap_byob_reads();
        let (result, controller) =
            pump_with_control(response, 4, &mut control, future::pending()).await;
        assert_eq!(finish_counting_mcap_byob_reads(), 1);
        assert!(matches!(result, Err(ExactLengthByobPumpError::Cancelled)));
        assert!(controller.signal().aborted());
    }

    #[wasm_bindgen_test]
    async fn zero_progress_malformed_and_detached_results_converge() {
        if !chrome_test_is_enabled() {
            return;
        }
        for (mode, expected_error) in [
            ("zero", ExactLengthByobPumpError::InvalidRangeBodyLength),
            ("malformed", ExactLengthByobPumpError::InvalidByobReadResult),
            ("detached", ExactLengthByobPumpError::DetachedReturnedView),
            (
                "oversized_backing",
                ExactLengthByobPumpError::InvalidByobReadResult,
            ),
        ] {
            patch_next_mcap_byob_read(mode);
            let response = make_mcap_byob_response(4, false, false).unchecked_into();
            let (result, controller) = pump(response, 4, future::pending()).await;
            assert!(matches!(result, Err(error) if error == expected_error));
            assert!(controller.signal().aborted());
        }
    }

    #[wasm_bindgen_test]
    async fn controlled_origin_fetch_exercises_the_production_pump_contract() {
        if !chrome_test_is_enabled() {
            return;
        }

        reset_production_pump_test_harness();
        let instrumentation = ProductionPumpInstrumentationGuard::start();
        let (root, range, work) = scopes();
        let baseline = root.accounting_scalar_snapshot();

        let exact = ControlledProductionPumpScenario::register("exact").await;
        let controller = web_sys::AbortController::new().expect("browser exposes AbortController");
        let response = exact.fetch(&controller).await;
        let reads_before = ProductionPumpInstrumentationGuard::stat("reads");
        let detached_before = ProductionPumpInstrumentationGuard::stat("detachedInputViews");
        let releases_before = ProductionPumpInstrumentationGuard::stat("releaseLockCalls");
        let cancels_before = ProductionPumpInstrumentationGuard::stat("cancelCalls");
        let mut body = pump_controlled_response(
            &response,
            &controller,
            &root,
            &range,
            &work,
            4,
            2,
            future::pending(),
        )
        .await
        .expect("exact controlled body succeeds");
        assert_eq!(body.as_slice(), &[10, 11, 12, 13]);
        assert!(!controller.signal().aborted());
        assert!(ProductionPumpInstrumentationGuard::stat("reads") >= reads_before + 3);
        assert!(ProductionPumpInstrumentationGuard::stat("detachedInputViews") > detached_before);
        assert_eq!(
            ProductionPumpInstrumentationGuard::stat("releaseLockCalls"),
            releases_before + 1
        );
        assert_eq!(
            ProductionPumpInstrumentationGuard::stat("cancelCalls"),
            cancels_before
        );
        assert_eq!(
            ProductionPumpInstrumentationGuard::stat("arrayBufferCalls"),
            0
        );
        let active_body = root.accounting_scalar_snapshot();
        assert_eq!(
            active_body.active_reservation_records,
            baseline.active_reservation_records + 1
        );
        assert!(active_body.active_retained_bytes > baseline.active_retained_bytes);
        let deallocations_before = super::fixed_output::completed_deallocation_count_for_test();
        let probe_root = root.clone();
        body.install_after_bytes_drop_probe(move || {
            assert_eq!(
                super::fixed_output::completed_deallocation_count_for_test(),
                deallocations_before + 1
            );
            assert_eq!(
                probe_root
                    .accounting_scalar_snapshot()
                    .active_reservation_records,
                baseline.active_reservation_records + 1
            );
        });
        drop(body);
        assert_eq!(root.accounting_scalar_snapshot(), baseline);
        exact.remove().await;

        let heartbeat = ControlledProductionPumpScenario::register("heartbeat").await;
        let controller = web_sys::AbortController::new().expect("browser exposes AbortController");
        let response = heartbeat.fetch(&controller).await;
        let heartbeat_before = ProductionPumpInstrumentationGuard::stat("heartbeat");
        begin_production_pump_active_window();
        let body = pump_controlled_response(
            &response,
            &controller,
            &root,
            &range,
            &work,
            64,
            1,
            future::pending(),
        )
        .await
        .expect("ready controlled body succeeds");
        let active_zero_timeout_calls = end_production_pump_active_window();
        assert!(
            active_zero_timeout_calls >= 64,
            "each of the 64 non-empty production slices must schedule a zero-delay timer"
        );
        assert!(ProductionPumpInstrumentationGuard::stat("heartbeat") > heartbeat_before);
        drop(body);
        assert_eq!(root.accounting_scalar_snapshot(), baseline);
        heartbeat.remove().await;

        assert_controlled_failure(
            "early_eof",
            ExactLengthByobPumpError::InvalidRangeBodyLength,
            future::pending(),
            false,
            &instrumentation,
            &root,
            &range,
            &work,
            baseline,
        )
        .await;
        assert_controlled_failure(
            "overlong",
            ExactLengthByobPumpError::InvalidRangeBodyLength,
            future::pending(),
            false,
            &instrumentation,
            &root,
            &range,
            &work,
            baseline,
        )
        .await;
        assert_controlled_failure(
            "infinite",
            ExactLengthByobPumpError::InvalidRangeBodyLength,
            future::pending(),
            true,
            &instrumentation,
            &root,
            &range,
            &work,
            baseline,
        )
        .await;
        assert_controlled_failure(
            "stall",
            ExactLengthByobPumpError::Timeout,
            re_async::sleep(Duration::from_millis(100)),
            true,
            &instrumentation,
            &root,
            &range,
            &work,
            baseline,
        )
        .await;

        let release_failure = ControlledProductionPumpScenario::register("release_failure").await;
        let controller = web_sys::AbortController::new().expect("browser exposes AbortController");
        let response = release_failure.fetch(&controller).await;
        fail_next_production_pump_release();
        let cancels_before = ProductionPumpInstrumentationGuard::stat("cancelCalls");
        let result = pump_controlled_response(
            &response,
            &controller,
            &root,
            &range,
            &work,
            4,
            2,
            future::pending(),
        )
        .await;
        assert!(matches!(result, Err(ExactLengthByobPumpError::ReadFailed)));
        assert!(controller.signal().aborted());
        assert!(ProductionPumpInstrumentationGuard::stat("cancelCalls") > cancels_before);
        assert_eq!(root.accounting_scalar_snapshot(), baseline);
        release_failure.remove().await;

        assert_eq!(
            ProductionPumpInstrumentationGuard::stat("arrayBufferCalls"),
            0
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nz(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).unwrap()
    }

    #[test]
    fn exact_body_requires_a_distinct_eof_probe() {
        let mut state = ExactLengthPumpState::new(nz(4));
        assert_eq!(state.requested_bytes(nz(3)), Some(nz(3)));
        assert_eq!(
            state.accept(false, 3, nz(3)),
            Ok(PumpStep::Copy {
                offset: 0,
                len: nz(3)
            })
        );
        assert_eq!(state.requested_bytes(nz(3)), Some(nz(1)));
        assert_eq!(
            state.accept(false, 1, nz(1)),
            Ok(PumpStep::Copy {
                offset: 3,
                len: nz(1)
            })
        );
        assert_eq!(state.requested_bytes(nz(3)), Some(NonZeroU64::MIN));
        assert_eq!(
            state.accept(true, 0, NonZeroU64::MIN),
            Ok(PumpStep::Complete)
        );
        assert_eq!(state.requested_bytes(nz(3)), None);
    }

    #[test]
    fn invalid_lengths_converge_without_advancing() {
        for (done, returned, expected) in [
            (true, 0, PumpStateError::EarlyEof),
            (false, 0, PumpStateError::ZeroProgress),
            (false, 3, PumpStateError::Overlong),
        ] {
            let mut state = ExactLengthPumpState::new(nz(2));
            assert_eq!(state.accept(done, returned, nz(2)), Err(expected));
        }

        let mut overlong = ExactLengthPumpState::new(nz(1));
        assert!(matches!(
            overlong.accept(false, 1, nz(1)),
            Ok(PumpStep::Copy { .. })
        ));
        assert_eq!(
            overlong.accept(false, 1, NonZeroU64::MIN),
            Err(PumpStateError::Overlong)
        );
    }
}
