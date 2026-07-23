use async_stream::stream;
use futures_core::stream::Stream;
use std::{ffi::c_void, pin::Pin, ptr::addr_of_mut};
use tokio::{
    io::{self, AsyncRead, AsyncWrite},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
};
use tonic::transport::server::Connected;
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::{CloseHandle, LocalFree, HLOCAL},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION,
            },
            GetTokenInformation, TokenLogonSid, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
            TOKEN_GROUPS, TOKEN_QUERY,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

#[allow(dead_code)]
struct UnsafeSecurityAttributes(SECURITY_ATTRIBUTES);

unsafe impl Send for UnsafeSecurityAttributes {}
unsafe impl Sync for UnsafeSecurityAttributes {}

impl UnsafeSecurityAttributes {
    fn as_mut_ptr(&mut self) -> *mut c_void {
        addr_of_mut!(self.0).cast()
    }
}

struct OwnedSecurityDescriptor(PSECURITY_DESCRIPTOR);

unsafe impl Send for OwnedSecurityDescriptor {}
unsafe impl Sync for OwnedSecurityDescriptor {}

impl OwnedSecurityDescriptor {
    fn as_ptr(&self) -> *mut c_void {
        self.0 .0
    }
}

impl Drop for OwnedSecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(HLOCAL(self.as_ptr()));
        }
    }
}

pub struct TonicNamedPipeServer {
    inner: NamedPipeServer,
}

impl Connected for TonicNamedPipeServer {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

impl AsyncRead for TonicNamedPipeServer {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for TonicNamedPipeServer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl TonicNamedPipeServer {
    pub fn new(path: &str) -> io::Result<impl Stream<Item = io::Result<TonicNamedPipeServer>>> {
        Self::new_with_first_pipe_callback(path, || {})
    }

    pub fn new_with_first_pipe_callback<F>(
        path: &str,
        on_first_pipe_created: F,
    ) -> io::Result<impl Stream<Item = io::Result<TonicNamedPipeServer>>>
    where
        F: FnOnce() + Send + 'static,
    {
        let name = path.to_string();
        let security_descriptor = create_pipe_security_descriptor()?;
        let mut security_attributes = UnsafeSecurityAttributes(SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security_descriptor.as_ptr(),
            bInheritHandle: false.into(),
        });

        Ok(stream! {
            // Keep the LocalAlloc-owned descriptor alive for every pipe instance.
            let _security_descriptor = security_descriptor;
            unsafe {
                let mut on_first_pipe_created = Some(on_first_pipe_created);
                let mut server = ServerOptions::new()
                    .first_pipe_instance(true)
                    .create_with_security_attributes_raw(
                        &name,
                        security_attributes.as_mut_ptr()
                    )?;
                if let Some(on_first_pipe_created) = on_first_pipe_created.take() {
                    on_first_pipe_created();
                }

                loop {
                    server.connect().await?;

                    let client = TonicNamedPipeServer {
                        inner: server,
                    };

                    yield Ok(client);

                    server = ServerOptions::new()
                        .create_with_security_attributes_raw(
                            &name,
                            security_attributes.as_mut_ptr()
                        )?;
                }
            }
        })
    }
}

fn create_pipe_security_descriptor() -> io::Result<OwnedSecurityDescriptor> {
    let logon_sid = current_logon_sid_string()?;
    let sddl = pipe_sddl(&logon_sid);
    let sddl_wide = sddl.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let mut security_descriptor = PSECURITY_DESCRIPTOR::default();

    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl_wide.as_ptr()),
            SDDL_REVISION,
            &mut security_descriptor,
            None,
        )
        .map_err(|error| {
            io::Error::other(format!(
                "failed to create named-pipe security descriptor: {error}"
            ))
        })?;
    }

    Ok(OwnedSecurityDescriptor(security_descriptor))
}

fn pipe_sddl(logon_sid: &str) -> String {
    // AppContainer access checks use both the caller identity and restricted
    // SID sets. The logon SID limits the normal identity to this login session
    // and lets the trusted server process create subsequent pipe instances.
    // AC/RC grant only read/write data plus synchronization (0x00100003), so a
    // sandboxed client can connect but cannot create a competing pipe instance.
    // Network access is explicitly denied.
    format!(
        "D:(D;;GA;;;NU)(A;;GA;;;SY)(A;;GRGW;;;{logon_sid})(A;;0x00100003;;;AC)(A;;0x00100003;;;RC)S:(ML;;NW;;;LW)"
    )
}

fn current_logon_sid_string() -> io::Result<String> {
    unsafe {
        let mut token = Default::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|error| {
            io::Error::other(format!("failed to open current process token: {error}"))
        })?;

        let result = logon_sid_string_from_token(token);
        let _ = CloseHandle(token);
        result
    }
}

fn logon_sid_string_from_token(token: windows::Win32::Foundation::HANDLE) -> io::Result<String> {
    unsafe {
        let mut token_info_length = 0;
        let _ = GetTokenInformation(token, TokenLogonSid, None, 0, &mut token_info_length);
        if token_info_length < size_of::<TOKEN_GROUPS>() as u32 {
            return Err(io::Error::other(
                "failed to get current logon SID buffer size",
            ));
        }

        let word_count = (token_info_length as usize).div_ceil(size_of::<usize>());
        let mut token_info = vec![0usize; word_count];
        GetTokenInformation(
            token,
            TokenLogonSid,
            Some(token_info.as_mut_ptr().cast()),
            token_info_length,
            &mut token_info_length,
        )
        .map_err(|error| io::Error::other(format!("failed to get current logon SID: {error}")))?;

        let token_groups = &*(token_info.as_ptr() as *const TOKEN_GROUPS);
        if token_groups.GroupCount != 1 {
            return Err(io::Error::other(format!(
                "expected one logon SID, got {}",
                token_groups.GroupCount
            )));
        }

        let mut sid_string = PWSTR::null();
        ConvertSidToStringSidW(token_groups.Groups[0].Sid, &mut sid_string).map_err(|error| {
            io::Error::other(format!("failed to convert current logon SID: {error}"))
        })?;
        let result = sid_string
            .to_string()
            .map_err(|error| io::Error::other(format!("failed to decode logon SID: {error}")));
        let _ = LocalFree(HLOCAL(sid_string.as_ptr().cast()));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{create_pipe_security_descriptor, pipe_sddl, UnsafeSecurityAttributes};
    use std::{
        os::windows::io::IntoRawHandle,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::net::windows::named_pipe::{NamedPipeClient, ServerOptions};
    use windows::{
        core::w,
        Win32::{
            Foundation::{CloseHandle, LocalFree, BOOL, ERROR_ACCESS_DENIED, HLOCAL},
            Security::{
                Authorization::ConvertStringSidToSidW, CheckTokenMembership, PSID,
                SECURITY_ATTRIBUTES, TOKEN_QUERY,
            },
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        },
    };

    fn current_token_has_network_sid() -> bool {
        unsafe {
            let mut token = Default::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).unwrap();

            let mut network_sid = PSID::default();
            ConvertStringSidToSidW(w!("S-1-5-2"), &mut network_sid).unwrap();
            let mut is_member = BOOL::default();
            let membership_result = CheckTokenMembership(token, network_sid, &mut is_member);

            let _ = LocalFree(HLOCAL(network_sid.0));
            let _ = CloseHandle(token);
            membership_result.unwrap();
            is_member.as_bool()
        }
    }

    #[test]
    fn pipe_sddl_is_limited_to_logon_session_and_sandboxed_clients() {
        let sddl = pipe_sddl("S-1-5-5-42-99");

        assert!(sddl.contains("(D;;GA;;;NU)"));
        assert!(sddl.contains("(A;;GRGW;;;S-1-5-5-42-99)"));
        assert!(sddl.contains("(A;;0x00100003;;;AC)"));
        assert!(sddl.contains("(A;;0x00100003;;;RC)"));
        assert!(sddl.contains("S:(ML;;NW;;;LW)"));
        assert!(!sddl.contains(";;;BU)"));
        assert!(!sddl.contains(";;;BA)"));
        assert!(!sddl.contains(";;;WD)"));
    }

    #[tokio::test]
    async fn secured_session_local_pipe_enforces_network_and_logon_access() {
        let security_descriptor = create_pipe_security_descriptor().unwrap();
        let mut security_attributes = UnsafeSecurityAttributes(SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security_descriptor.as_ptr(),
            bInheritHandle: false.into(),
        });
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let pipe_path = format!(
            r"\\.\pipe\LOCAL\azookey_security_test_{}_{}",
            std::process::id(),
            nonce
        );

        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .create_with_security_attributes_raw(&pipe_path, security_attributes.as_mut_ptr())
                .unwrap()
        };
        let client_handle = shared::open_named_pipe_client_handle(&pipe_path);
        // The VM test runner connects through OpenSSH and therefore carries the
        // NETWORK SID. Production explicitly denies that token; interactive
        // and service runners exercise the same-logon success path below.
        if current_token_has_network_sid() {
            let error = match client_handle {
                Ok(_) => panic!("network token unexpectedly connected to local-only pipe"),
                Err(error) => error,
            };
            assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED.0 as i32));
            return;
        }

        let client_handle = client_handle.unwrap();
        let _client =
            unsafe { NamedPipeClient::from_raw_handle(client_handle.into_raw_handle()) }.unwrap();
        server.connect().await.unwrap();

        assert!(ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_path)
            .is_err());
    }
}
