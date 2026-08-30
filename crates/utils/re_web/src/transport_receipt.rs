//! Strict producer-issued transport receipts for Web remote-MCAP operations.
//!
//! The receipt is the only adapter-facing handoff from the strict Range fetch producer. It
//! owns the completed body and the non-authority Web correlation material. It never exposes
//! the raw body, the browser `Response`, the reader, the `AbortController`, the URL, the
//! `ETag`, or the validator.

use re_mcap_web_contract::WebCorrelationMaterialV1;

mod private {
    /// Seals [`TransportBodyOwner`] so only this crate's authenticated body owners can
    /// implement it.
    pub trait Sealed {}
}

/// The completed body owner held inside a [`RemoteTransportReceiptV1`].
///
/// The trait is sealed: downstream crates cannot implement it for their own types. It exists
/// only so the receipt core can stay generic and therefore testable on host builds while the
/// production concrete owner (`ExactLengthRangeBody`) remains Web-only.
pub trait TransportBodyOwner: private::Sealed {
    /// Returns the owned body bytes for the synchronous consume callback.
    fn as_slice(&self) -> &[u8];
}

/// A producer-issued, move-only transport receipt for one exact-length body read.
///
/// The receipt is issued only by the strict Range fetch producer after full validation. It
/// owns the completed body and the non-authority Web correlation material. Consuming it runs
/// a synchronous, non-escaping body callback exactly once.
///
/// # Construction
///
/// In this substage the concrete Web-only issuer is deferred to the adapter substage, where
/// the public issue seam gains a real production caller. Until then the receipt is
/// constructed only by in-crate test code that has access to the private fields. The private
/// fields keep downstream crates from constructing or decomposing the receipt.
pub struct RemoteTransportReceiptV1<B: TransportBodyOwner> {
    body: B,
    material: WebCorrelationMaterialV1,
}

/// Opaque transport-receipt consumption error.
///
/// The receipt itself never fabricates this error; it only forwards the callback's error
/// transparently. It exposes no body, URL, `ETag`, or validator detail.
///
/// # Forward constraint
///
/// The type is `#[non_exhaustive]` with a private field, so downstream crates cannot
/// construct it. The adapter substage must therefore provide an adapter-facing error
/// construction/forwarding path (or map transport errors into its own opaque error) before
/// physical-error handling can be composed.
#[non_exhaustive]
pub struct TransportReceiptErrorV1 {
    _private: (),
}

impl<B: TransportBodyOwner> RemoteTransportReceiptV1<B> {
    /// Consumes the receipt exactly once, running the body callback and returning the
    /// non-authority correlation material for the future adapter match step.
    ///
    /// # Non-escape
    ///
    /// The `for<'body>` higher-ranked bound prevents the callback from returning or storing
    /// the borrowed body slice: `R` cannot mention `'body`, so a value such as
    /// `&'static [u8]` cannot be produced from `&'body [u8]`. This is enforced by the type
    /// system itself rather than by a compile-fail doctest.
    ///
    /// # One-shot
    ///
    /// Consuming the receipt by value prevents reuse: the receipt is moved into this method
    /// and cannot be called a second time.
    pub fn consume_transport_v1<R>(
        self,
        callback: impl for<'body> FnOnce(&'body [u8]) -> Result<R, TransportReceiptErrorV1>,
    ) -> Result<(R, WebCorrelationMaterialV1), TransportReceiptErrorV1> {
        let Self { body, material } = self;
        let result = callback(body.as_slice());
        drop(body);
        match result {
            Ok(value) => Ok((value, material)),
            Err(err) => Err(err),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl private::Sealed for crate::chrome_byob::ExactLengthRangeBody {}

#[cfg(target_arch = "wasm32")]
impl TransportBodyOwner for crate::chrome_byob::ExactLengthRangeBody {
    fn as_slice(&self) -> &[u8] {
        crate::chrome_byob::ExactLengthRangeBody::as_slice(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use re_mcap_web_contract::CorrelationFactoryV1;
    use static_assertions::assert_not_impl_any;
    use std::cell::RefCell;
    use std::rc::Rc;

    type DropLog = Rc<RefCell<Vec<&'static str>>>;

    struct BodyBytes {
        data: Vec<u8>,
        log: DropLog,
    }

    impl BodyBytes {
        fn as_slice(&self) -> &[u8] {
            &self.data
        }
    }

    impl Drop for BodyBytes {
        fn drop(&mut self) {
            self.log.borrow_mut().push("body");
        }
    }

    struct AccountingProbe {
        log: DropLog,
    }

    impl Drop for AccountingProbe {
        fn drop(&mut self) {
            self.log.borrow_mut().push("accounting");
        }
    }

    struct MockBodyOwner {
        bytes: Option<BodyBytes>,
        accounting: Option<AccountingProbe>,
    }

    impl MockBodyOwner {
        fn new(data: Vec<u8>, log: DropLog) -> Self {
            Self {
                bytes: Some(BodyBytes {
                    data,
                    log: log.clone(),
                }),
                accounting: Some(AccountingProbe { log }),
            }
        }
    }

    impl Drop for MockBodyOwner {
        fn drop(&mut self) {
            // Mirror the production bytes-before-accounting release order.
            drop(self.bytes.take());
            drop(self.accounting.take());
        }
    }

    impl private::Sealed for MockBodyOwner {}

    impl TransportBodyOwner for MockBodyOwner {
        fn as_slice(&self) -> &[u8] {
            self.bytes
                .as_ref()
                .expect("a live mock body owns its bytes")
                .as_slice()
        }
    }

    assert_not_impl_any!(
        RemoteTransportReceiptV1<MockBodyOwner>: Clone, Copy, core::fmt::Debug, std::hash::Hash
    );
    assert_not_impl_any!(TransportReceiptErrorV1: Clone, Copy, core::fmt::Debug, std::hash::Hash);

    fn issue_mock(data: Vec<u8>, log: DropLog) -> RemoteTransportReceiptV1<MockBodyOwner> {
        let (_pair, web_permit, _mcap_permit) = CorrelationFactoryV1::new_operation_v1();
        let material = web_permit.into_material_v1();
        RemoteTransportReceiptV1 {
            body: MockBodyOwner::new(data, log),
            material,
        }
    }

    fn test_error() -> TransportReceiptErrorV1 {
        TransportReceiptErrorV1 { _private: () }
    }

    #[test]
    fn consume_returns_owned_value_and_material() {
        let log = DropLog::default();
        let receipt = issue_mock(vec![1, 2, 3], log.clone());
        let (len, material) = receipt
            .consume_transport_v1(|body| Ok::<_, TransportReceiptErrorV1>(body.len()))
            .unwrap_or_else(|_| panic!("consume_transport_v1 should succeed"));
        assert_eq!(len, 3);
        let _ = material;
    }

    #[test]
    fn consume_passes_exact_body_bytes() {
        let log = DropLog::default();
        let receipt = issue_mock(vec![7, 8, 9, 10], log.clone());
        let (seen, _material) = receipt
            .consume_transport_v1(|body| Ok::<_, TransportReceiptErrorV1>(body.to_vec()))
            .unwrap_or_else(|_| panic!("consume_transport_v1 should succeed"));
        assert_eq!(seen, vec![7, 8, 9, 10]);
    }

    #[test]
    fn consume_drops_body_after_callback() {
        let log = DropLog::default();
        let receipt = issue_mock(vec![1], log.clone());
        assert!(log.borrow().is_empty());
        let _ = receipt
            .consume_transport_v1(|body| Ok::<_, TransportReceiptErrorV1>(body.len()))
            .unwrap_or_else(|_| panic!("consume_transport_v1 should succeed"));
        assert_eq!(&*log.borrow(), &["body", "accounting"]);
    }

    #[test]
    fn unconsumed_receipt_drops_body_before_accounting() {
        let log = DropLog::default();
        let receipt = issue_mock(vec![1, 2], log.clone());
        assert!(log.borrow().is_empty());
        drop(receipt);
        assert_eq!(&*log.borrow(), &["body", "accounting"]);
    }

    #[test]
    fn callback_error_is_transparent_and_material_is_consumed() {
        let log = DropLog::default();
        let receipt = issue_mock(vec![1], log.clone());
        let result = receipt.consume_transport_v1(|_body| Err::<usize, _>(test_error()));
        assert!(result.is_err());
        assert_eq!(&*log.borrow(), &["body", "accounting"]);
    }
}
