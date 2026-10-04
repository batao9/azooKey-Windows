// RPC authorization propagates Tonic's prescribed error type without boxing.
#![allow(clippy::result_large_err)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tonic::{Request, Status};

/// Assigned by the pipe transport, never supplied by RPC metadata.
#[derive(Debug)]
pub struct PipeClient {
    connected: AtomicBool,
    management_allowed: bool,
}

impl PipeClient {
    pub(crate) fn new(management_allowed: bool) -> Arc<Self> {
        Arc::new(Self {
            connected: AtomicBool::new(true),
            management_allowed,
        })
    }

    pub(crate) fn disconnect(&self) {
        self.connected.store(false, Ordering::Release);
    }

    fn from_request<T>(request: &Request<T>) -> Result<&Arc<Self>, Status> {
        let client = request
            .extensions()
            .get::<Arc<Self>>()
            .ok_or_else(|| Status::unauthenticated("missing pipe connection identity"))?;
        if !client.connected.load(Ordering::Acquire) {
            return Err(Status::unavailable("pipe connection closed"));
        }
        Ok(client)
    }

    pub fn authorize_management<T>(request: &Request<T>) -> Result<(), Status> {
        if Self::from_request(request)?.management_allowed {
            Ok(())
        } else {
            Err(Status::permission_denied(
                "management RPC requires a desktop logon client",
            ))
        }
    }

    pub fn authorize_connection<T>(request: &Request<T>) -> Result<(), Status> {
        Self::from_request(request).map(|_| ())
    }
}

/// Accessed only while holding the converter's mutation mutex. Empty text can
/// still have snapshots/pinned learning candidates, so only ClearText releases.
#[derive(Debug, Default)]
pub struct CompositionOwner {
    owner: Option<Arc<PipeClient>>,
}

impl CompositionOwner {
    pub fn authorize<T>(
        &mut self,
        request: &Request<T>,
        claim: bool,
        reset: impl FnOnce(),
    ) -> Result<(), Status> {
        let client = PipeClient::from_request(request)?;
        if let Some(owner) = &self.owner {
            if !owner.connected.load(Ordering::Acquire) {
                // Remaining request/ConnectInfo clones must not keep an abandoned
                // composition alive. Erase it before the next client can read it.
                reset();
                self.owner = None;
            } else if !Arc::ptr_eq(owner, client) {
                return Err(Status::permission_denied(
                    "composition belongs to another connection",
                ));
            }
        }
        if claim && self.owner.is_none() {
            self.owner = Some(Arc::clone(client));
        }
        Ok(())
    }

    pub fn release(&mut self) {
        self.owner = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::Code;

    fn request(client: &Arc<PipeClient>) -> Request<()> {
        let mut request = Request::new(());
        request.extensions_mut().insert(Arc::clone(client));
        request
    }

    #[test]
    fn owner_is_connection_bound_until_clear_or_disconnect() {
        let first = PipeClient::new(true);
        let second = PipeClient::new(true);
        let mut owner = CompositionOwner::default();
        owner
            .authorize(&request(&first), true, || panic!("not abandoned"))
            .unwrap();
        for claim in [false, true] {
            assert_eq!(
                owner
                    .authorize(&request(&second), claim, || panic!(
                        "cannot reset another owner"
                    ))
                    .unwrap_err()
                    .code(),
                Code::PermissionDenied
            );
        }
        owner
            .authorize(&request(&first), false, || panic!("same owner"))
            .unwrap();
        owner.release();
        owner
            .authorize(&request(&second), true, || panic!("already cleared"))
            .unwrap();

        // A retained Arc (e.g. an old in-flight request) is not a live transport.
        second.disconnect();
        let mut erased = false;
        owner
            .authorize(&request(&first), true, || erased = true)
            .unwrap();
        assert!(erased);
        assert_eq!(
            owner
                .authorize(&request(&second), true, || {})
                .unwrap_err()
                .code(),
            Code::Unavailable
        );
    }

    #[test]
    fn unclaimed_clear_does_not_block_next_client_and_identity_is_required() {
        let mut owner = CompositionOwner::default();
        let first = PipeClient::new(true);
        let second = PipeClient::new(false);
        owner
            .authorize(&request(&first), false, || panic!("no stale state"))
            .unwrap();
        owner
            .authorize(&request(&second), true, || panic!("no stale state"))
            .unwrap();
        assert_eq!(
            owner
                .authorize(&Request::new(()), true, || {})
                .unwrap_err()
                .code(),
            Code::Unauthenticated
        );
        assert!(PipeClient::authorize_management(&request(&first)).is_ok());
        assert_eq!(
            PipeClient::authorize_management(&request(&second))
                .unwrap_err()
                .code(),
            Code::PermissionDenied
        );
    }
}
