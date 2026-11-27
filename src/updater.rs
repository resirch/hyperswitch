use semver::Version;
use serde::Deserialize;
use std::ffi::c_void;
use std::process::Command;
use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::{
    WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_ADDREQ_FLAG_REPLACE,
    WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_API_HOST: PCWSTR = w!("api.github.com");
const GITHUB_RELEASE_PATH: PCWSTR = w!("/repos/resirch/hyperswitch/releases/latest");
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
}

#[derive(Debug)]
struct AvailableUpdate {
    version: Version,
    html_url: String,
}

struct WinHttpHandle(*mut c_void);

impl WinHttpHandle {
    fn new(raw: *mut c_void) -> Option<Self> {
        if raw.is_null() {
            None
        } else {
            Some(Self(raw))
        }
    }
}

impl Drop for WinHttpHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// Spawn a background thread to check GitHub for a newer release and prompt the
/// user to open the release page if one is available.
pub fn spawn_update_check(enabled: bool) {
    if !enabled {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(|| {
            let _ = maybe_prompt_for_update();
        });
}

fn maybe_prompt_for_update() -> Option<()> {
    let update = check_for_update()?;

    if prompt_for_update(&update) {
        open_release_page(&update.html_url);
    }

    Some(())
}

fn check_for_update() -> Option<AvailableUpdate> {
    let body = fetch_latest_release_json()?;
    let release: GitHubRelease = serde_json::from_slice(&body).ok()?;
    let version = newer_release_version(&release.tag_name, CURRENT_VERSION)?;

    if !is_safe_release_url(&release.html_url) {
        return None;
    }

    Some(AvailableUpdate {
        version,
        html_url: release.html_url,
    })
}

fn fetch_latest_release_json() -> Option<Vec<u8>> {
    fetch_github_json(GITHUB_RELEASE_PATH)
}

fn fetch_github_json(path: PCWSTR) -> Option<Vec<u8>> {
    unsafe {
        let user_agent = utf16_null(&format!("hyperswitch/{CURRENT_VERSION}"));
        let session = WinHttpHandle::new(WinHttpOpen(
            PCWSTR(user_agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;

        let _ = WinHttpSetTimeouts(session.0, 5_000, 5_000, 5_000, 10_000);

        let connect = WinHttpHandle::new(WinHttpConnect(
            session.0,
            GITHUB_API_HOST,
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ))?;

        let request = WinHttpHandle::new(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            path,
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;

        let accept_header: Vec<u16> = "Accept: application/vnd.github+json\r\n"
            .encode_utf16()
            .collect();
        WinHttpAddRequestHeaders(
            request.0,
            &accept_header,
            WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE,
        )
        .ok()?;

        WinHttpSendRequest(request.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).ok()?;

        let mut status_code: u32 = 0;
        let mut status_len = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status_code as *mut u32).cast()),
            &mut status_len,
            std::ptr::null_mut(),
        )
        .ok()?;

        if status_code != 200 {
            return None;
        }

        let mut body = Vec::new();
        loop {
            let mut available: u32 = 0;
            WinHttpQueryDataAvailable(request.0, &mut available).ok()?;
            if available == 0 {
                break;
            }

            let to_read = available as usize;
            if body.len().saturating_add(to_read) > MAX_RESPONSE_BYTES {
                return None;
            }

            let start = body.len();
            body.resize(start + to_read, 0);

            let mut bytes_read: u32 = 0;
            WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr().cast(),
                available,
                &mut bytes_read,
            )
            .ok()?;

            body.truncate(start + bytes_read as usize);
            if bytes_read == 0 {
                break;
            }
        }

        Some(body)
    }
}

fn newer_release_version(tag_name: &str, current_version: &str) -> Option<Version> {
    let release_version = parse_version_tag(tag_name)?;
    let current_version = Version::parse(current_version).ok()?;

    if release_version > current_version {
        Some(release_version)
    } else {
        None
    }
}

fn parse_version_tag(tag_name: &str) -> Option<Version> {
    let normalized = tag_name
        .trim()
        .trim_start_matches(|ch| ch == 'v' || ch == 'V');
    Version::parse(normalized).ok()
}

fn is_safe_release_url(url: &str) -> bool {
    url.starts_with("https://github.com/")
}

fn prompt_for_update(update: &AvailableUpdate) -> bool {
    let title = utf16_null("Hyperswitch Update");
    let message = utf16_null(&format!(
        "Hyperswitch v{} is available.\nYou are running v{}.\n\nOpen the GitHub release page to update?",
        update.version, CURRENT_VERSION
    ));

    let choice = unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };

    choice == IDYES
}

fn open_release_page(url: &str) {
    let _ = Command::new("explorer").arg(url).spawn();
}

fn utf16_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{is_safe_release_url, newer_release_version, parse_version_tag, GitHubRelease};

    #[test]
    fn parses_v_prefixed_release_tags() {
        let version = parse_version_tag("v0.2.0").unwrap();
        assert_eq!(version.to_string(), "0.2.0");

        let upper = parse_version_tag(" V1.2.3 ").unwrap();
        assert_eq!(upper.to_string(), "1.2.3");
    }

    #[test]
    fn detects_newer_release_versions() {
        let version = newer_release_version("v0.2.0", "0.1.0").unwrap();
        assert_eq!(version.to_string(), "0.2.0");
    }

    #[test]
    fn ignores_equal_or_older_release_versions() {
        assert!(newer_release_version("v0.1.0", "0.1.0").is_none());
        assert!(newer_release_version("v0.0.9", "0.1.0").is_none());
    }

    #[test]
    fn parses_github_release_payload() {
        let json = br#"{"tag_name":"v0.2.0","html_url":"https://github.com/resirch/hyperswitch/releases/tag/v0.2.0","draft":false}"#;
        let release: GitHubRelease = serde_json::from_slice(json).unwrap();
        assert_eq!(
            newer_release_version(&release.tag_name, "0.1.0")
                .unwrap()
                .to_string(),
            "0.2.0"
        );
        assert!(is_safe_release_url(&release.html_url));
    }

    #[test]
    fn validates_github_release_urls() {
        assert!(is_safe_release_url(
            "https://github.com/resirch/hyperswitch/releases/tag/v0.2.0"
        ));
        assert!(!is_safe_release_url("file:///C:/Windows/System32/calc.exe"));
        assert!(!is_safe_release_url("http://github.com/resirch/hyperswitch"));
    }
}
