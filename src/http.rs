//! Minimal HTTPS GET over WinHTTP (no TLS crates, no C toolchain needed).

use windows::core::*;
use windows::Win32::Networking::WinHttp::*;

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
    /// The `Location` header of a redirect (only seen with `get_no_redirect`).
    pub location: Option<String>,
}

/// Largest body read (the installer download is the biggest, ~2.5 MB).
const MAX_BODY: usize = 64 * 1024 * 1024;

struct H(*mut core::ffi::c_void);
impl Drop for H {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

pub fn get(host: &str, path: &str) -> Option<Response> {
    request(host, path, true)
}

/// GET without following redirects, to read where a redirect points.
pub fn get_no_redirect(host: &str, path: &str) -> Option<Response> {
    request(host, path, false)
}

fn request(host: &str, path: &str, follow: bool) -> Option<Response> {
    unsafe {
        let session = H(WinHttpOpen(
            w!("WindyLand/0.1 (https://github.com/shabab239/WindyLand)"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return None;
        }
        let _ = WinHttpSetTimeouts(session.0, 5000, 5000, 8000, 10000);
        let host_w = HSTRING::from(host);
        let conn = H(WinHttpConnect(session.0, &host_w, 443, 0));
        if conn.0.is_null() {
            return None;
        }
        let path_w = HSTRING::from(path);
        let req = H(WinHttpOpenRequest(
            conn.0,
            w!("GET"),
            &path_w,
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if req.0.is_null() {
            return None;
        }
        if !follow {
            let off = WINHTTP_DISABLE_REDIRECTS.to_le_bytes();
            let _ = WinHttpSetOption(Some(req.0), WINHTTP_OPTION_DISABLE_FEATURE, Some(&off));
        }
        WinHttpSendRequest(req.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).ok()?;

        let mut status: u32 = 0;
        let mut len = 4u32;
        let mut idx = 0u32;
        let _ = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut len,
            &mut idx,
        );

        let mut location = None;
        if !follow {
            let mut buf = [0u16; 1024];
            let mut blen = (buf.len() * 2) as u32;
            let mut idx = 0u32;
            if WinHttpQueryHeaders(req.0, WINHTTP_QUERY_LOCATION, PCWSTR::null(), Some(buf.as_mut_ptr() as *mut _), &mut blen, &mut idx).is_ok() {
                location = Some(String::from_utf16_lossy(&buf[..(blen as usize / 2).min(buf.len())]));
            }
        }

        let mut body = Vec::new();
        loop {
            let mut avail = 0u32;
            if WinHttpQueryDataAvailable(req.0, &mut avail).is_err() || avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(req.0, body[start..].as_mut_ptr() as _, avail, &mut read).is_err() {
                body.truncate(start);
                break;
            }
            body.truncate(start + read as usize);
            if body.len() > MAX_BODY {
                break;
            }
        }
        Some(Response { status, body, location })
    }
}

/// Percent-encode a query component.
pub fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
