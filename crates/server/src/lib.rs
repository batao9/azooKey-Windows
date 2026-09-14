use async_stream::try_stream;
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
            FreeSid, GetTokenInformation,
            Isolation::{
                DeriveAppContainerSidFromAppContainerName, GetAppContainerNamedObjectPath,
            },
            TokenLogonSid, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_GROUPS,
            TOKEN_QUERY,
        },
        System::{
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    },
};

const LOCAL_PIPE_PREFIX: &str = r"\\.\pipe\LOCAL\";
const SEARCH_HOST_PACKAGE_FAMILY_NAME: &str = "MicrosoftWindows.Client.CBS_cw5n1h2txyewy";
const APPCONTAINER_PIPE_ACCESS_MASK: u32 = 0x00100083;

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

struct OwnedSid(PSID);

impl Drop for OwnedSid {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeSid(self.0);
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
        let (search_host_sid, search_host_namespace) = search_host_appcontainer()?;
        let search_host_name = appcontainer_pipe_path(path, &search_host_namespace)?;
        let security_descriptor = create_pipe_security_descriptor(None)?;
        let search_host_security_descriptor =
            create_pipe_security_descriptor(Some(&search_host_sid))?;
        let mut security_attributes = UnsafeSecurityAttributes(SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security_descriptor.as_ptr(),
            bInheritHandle: false.into(),
        });
        let mut search_host_security_attributes = UnsafeSecurityAttributes(SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: search_host_security_descriptor.as_ptr(),
            bInheritHandle: false.into(),
        });

        let mut server =
            unsafe { create_named_pipe_server(&name, &mut security_attributes, true)? };
        let mut search_host_server = unsafe {
            create_named_pipe_server(
                &search_host_name,
                &mut search_host_security_attributes,
                true,
            )?
        };
        on_first_pipe_created();

        Ok(try_stream! {
            // Keep the LocalAlloc-owned descriptor alive for every pipe instance.
            let _security_descriptor = security_descriptor;
            let _search_host_security_descriptor = search_host_security_descriptor;
            loop {
                let search_host_connected = tokio::select! {
                    result = server.connect() => result.map(|_| false),
                    result = search_host_server.connect() => result.map(|_| true),
                }?;
                let (connected, replacement) = if search_host_connected {
                    let replacement = unsafe {
                        create_named_pipe_server(
                            &search_host_name,
                            &mut search_host_security_attributes,
                            false,
                        )?
                    };
                    (&mut search_host_server, replacement)
                } else {
                    let replacement = unsafe {
                        create_named_pipe_server(&name, &mut security_attributes, false)?
                    };
                    (&mut server, replacement)
                };
                let client = std::mem::replace(connected, replacement);
                yield TonicNamedPipeServer { inner: client };
            }
        })
    }
}

unsafe fn create_named_pipe_server(
    name: &str,
    security_attributes: &mut UnsafeSecurityAttributes,
    first_pipe_instance: bool,
) -> io::Result<NamedPipeServer> {
    ServerOptions::new()
        .first_pipe_instance(first_pipe_instance)
        .create_with_security_attributes_raw(name, security_attributes.as_mut_ptr())
}

fn create_pipe_security_descriptor(
    search_host_sid: Option<&str>,
) -> io::Result<OwnedSecurityDescriptor> {
    let logon_sid = current_logon_sid_string()?;
    let sddl = pipe_sddl(&logon_sid, search_host_sid);
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

fn pipe_sddl(logon_sid: &str, search_host_sid: Option<&str>) -> String {
    // AppContainer access checks use both the caller identity and restricted
    // SID sets. Preserve desktop sandbox access on the existing LOCAL endpoint;
    // the package endpoint grants only the exact SearchHost SID. Neither sandbox
    // ACE grants FILE_CREATE_PIPE_INSTANCE (0x4). Network access is denied.
    let sandbox_sids = match search_host_sid {
        Some(sid) => vec![sid],
        None => vec!["AC", "RC"],
    };
    let sandbox_aces = sandbox_sids
        .iter()
        .map(|sid| format!("(A;;{APPCONTAINER_PIPE_ACCESS_MASK:#010x};;;{sid})"))
        .collect::<String>();
    format!("D:(D;;GA;;;NU)(A;;GA;;;SY)(A;;GRGW;;;{logon_sid}){sandbox_aces}S:(ML;;NW;;;LW)")
}

fn search_host_appcontainer() -> io::Result<(String, String)> {
    let package_family_name = SEARCH_HOST_PACKAGE_FAMILY_NAME
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let sid = OwnedSid(unsafe {
        DeriveAppContainerSidFromAppContainerName(PCWSTR(package_family_name.as_ptr())).map_err(
            |error| io::Error::other(format!("failed to derive SearchHost SID: {error}")),
        )?
    });
    let sid_string = sid_string(sid.0)?;

    let mut path_length = 0;
    let _ = unsafe {
        GetAppContainerNamedObjectPath(
            windows::Win32::Foundation::HANDLE::default(),
            sid.0,
            None,
            &mut path_length,
        )
    };
    if path_length == 0 {
        return Err(io::Error::other(
            "failed to get SearchHost named-object path length",
        ));
    }

    let mut path = vec![0_u16; path_length as usize];
    unsafe {
        GetAppContainerNamedObjectPath(
            windows::Win32::Foundation::HANDLE::default(),
            sid.0,
            Some(&mut path),
            &mut path_length,
        )
        .map_err(|error| {
            io::Error::other(format!(
                "failed to get SearchHost named-object path: {error}"
            ))
        })?;
    }
    let end = path
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(path.len());
    let namespace = String::from_utf16(&path[..end]).map_err(|error| {
        io::Error::other(format!(
            "failed to decode SearchHost named-object path: {error}"
        ))
    })?;

    // This API returns a session-relative object path, whereas desktop named
    // pipes need the fully qualified session namespace used by AppContainers.
    let mut session_id = 0;
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session_id) }
        .map_err(|error| io::Error::other(format!("failed to get pipe session: {error}")))?;
    Ok((sid_string, format!(r"Sessions\{session_id}\{namespace}")))
}

fn appcontainer_pipe_path(local_path: &str, namespace: &str) -> io::Result<String> {
    let leaf = local_path.strip_prefix(LOCAL_PIPE_PREFIX).ok_or_else(|| {
        io::Error::other(format!("expected LOCAL named-pipe path, got {local_path}"))
    })?;
    if leaf.is_empty() || leaf.contains('\\') {
        return Err(io::Error::other(format!(
            "expected named-pipe leaf after LOCAL prefix, got {local_path}"
        )));
    }

    Ok(format!(r"\\.\pipe\{}\{leaf}", namespace.trim_matches('\\')))
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

        sid_string(token_groups.Groups[0].Sid)
    }
}

fn sid_string(sid: PSID) -> io::Result<String> {
    unsafe {
        let mut value = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut value)
            .map_err(|error| io::Error::other(format!("failed to convert SID: {error}")))?;
        let result = value
            .to_string()
            .map_err(|error| io::Error::other(format!("failed to decode SID: {error}")));
        let _ = LocalFree(HLOCAL(value.as_ptr().cast()));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{
        appcontainer_pipe_path, create_pipe_security_descriptor, pipe_sddl,
        search_host_appcontainer, TonicNamedPipeServer, UnsafeSecurityAttributes,
        APPCONTAINER_PIPE_ACCESS_MASK,
    };
    use std::{
        os::windows::io::IntoRawHandle,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::net::windows::named_pipe::{NamedPipeClient, ServerOptions};
    use windows::{
        core::w,
        Win32::{
            Foundation::{LocalFree, BOOL, ERROR_ACCESS_DENIED, HANDLE, HLOCAL},
            Security::{
                Authorization::ConvertStringSidToSidW, CheckTokenMembership, PSID,
                SECURITY_ATTRIBUTES,
            },
        },
    };

    fn current_token_has_network_sid() -> bool {
        unsafe {
            let mut network_sid = PSID::default();
            ConvertStringSidToSidW(w!("S-1-5-2"), &mut network_sid).unwrap();
            let mut is_member = BOOL::default();
            // A null token handle makes Windows evaluate the calling thread's
            // effective token, duplicating its primary token when necessary.
            let membership_result =
                CheckTokenMembership(HANDLE::default(), network_sid, &mut is_member);

            let _ = LocalFree(HLOCAL(network_sid.0));
            membership_result.unwrap();
            is_member.as_bool()
        }
    }

    #[test]
    fn pipe_sddl_is_limited_to_logon_session_and_search_host() {
        let search_host_sid = "S-1-15-2-42-99";
        let sddl = pipe_sddl("S-1-5-5-42-99", Some(search_host_sid));

        assert!(sddl.contains("(D;;GA;;;NU)"));
        assert!(sddl.contains("(A;;GRGW;;;S-1-5-5-42-99)"));
        assert!(sddl.contains("(A;;0x00100083;;;S-1-15-2-42-99)"));
        assert_eq!(APPCONTAINER_PIPE_ACCESS_MASK & 0x4, 0);
        assert!(sddl.contains("S:(ML;;NW;;;LW)"));
        assert!(!sddl.contains(";;;AC)"));
        assert!(!sddl.contains(";;;RC)"));
        assert!(!sddl.contains(";;;BU)"));
        assert!(!sddl.contains(";;;BA)"));
        assert!(!sddl.contains(";;;WD)"));
    }

    #[test]
    fn desktop_pipe_sddl_preserves_restricted_client_access() {
        let sddl = pipe_sddl("S-1-5-5-42-99", None);
        assert!(sddl.contains("(A;;0x00100083;;;AC)"));
        assert!(sddl.contains("(A;;0x00100083;;;RC)"));
        assert!(sddl.contains("(A;;GRGW;;;S-1-5-5-42-99)"));
        assert!(sddl.contains("(D;;GA;;;NU)"));
    }

    #[test]
    fn appcontainer_pipe_path_uses_the_package_namespace_and_local_leaf() {
        let path = appcontainer_pipe_path(
            r"\\.\pipe\LOCAL\azookey_server",
            r"\Sessions\7\AppContainerNamedObjects\S-1-15-2-42",
        )
        .unwrap();

        assert_eq!(
            path,
            r"\\.\pipe\Sessions\7\AppContainerNamedObjects\S-1-15-2-42\azookey_server"
        );
    }

    #[tokio::test]
    async fn pipe_listeners_reserve_both_names_and_release_them_on_shutdown() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let local_path = format!(
            r"\\.\pipe\LOCAL\azookey_listener_test_{}_{nonce}",
            std::process::id()
        );
        let (_, namespace) = search_host_appcontainer().unwrap();
        let mut session_id = 0;
        unsafe { super::ProcessIdToSessionId(std::process::id(), &mut session_id) }.unwrap();
        assert!(namespace.starts_with(&format!(
            r"Sessions\{session_id}\AppContainerNamedObjects\S-1-15-2-"
        )));
        let package_path = appcontainer_pipe_path(&local_path, &namespace).unwrap();

        for _ in 0..2 {
            let incoming = TonicNamedPipeServer::new(&local_path).unwrap();
            for path in [&local_path, &package_path] {
                assert!(
                    ServerOptions::new()
                        .first_pipe_instance(true)
                        .create(path)
                        .is_err(),
                    "listener did not reserve {path}"
                );
            }
            drop(incoming);
        }
    }

    #[tokio::test]
    async fn secured_session_local_pipe_enforces_network_and_logon_access() {
        let security_descriptor = create_pipe_security_descriptor(None).unwrap();
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
