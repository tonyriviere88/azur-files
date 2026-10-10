//! WinHTTP: one GET, with the body handed to the caller as it arrives.
//!
//! The Windows half of [`crate::update`].
//!
//! `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY` is the reason this is WinHTTP and not a crate: it is the
//! machine's own proxy configuration — WPAD, a PAC script, whatever policy set — and the
//! certificates are the machine's store, so a laptop behind a corporate proxy reaches GitHub the
//! way its browser does. Redirects are WinHTTP's own business and are followed, which a release
//! asset needs (its URL answers with a redirect to a download host); the default policy never
//! follows one from `https` down to `http`.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_CONTENT_LENGTH,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

use super::split_url;

/// A WinHTTP handle, closed when it goes. Declared session, connection, request, so they drop in
/// the reverse order, which is the order WinHTTP wants them closed in.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle WinHTTP gave out, closed once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// What went wrong, in words where the error is one somebody could do something about.
fn failed(what: &str) -> String {
    // SAFETY: no preconditions.
    let code = unsafe { GetLastError() };
    match code {
        12002 => "GitHub did not answer in time".into(),
        12007 | 12029 => "Could not reach GitHub — no network, or a proxy in the way".into(),
        12175 => "The connection to GitHub could not be secured".into(),
        _ => format!("{what} failed (WinHTTP error {code})"),
    }
}

pub(super) fn get(
    url: &str,
    accept: &str,
    sink: &mut super::Sink<'_>,
) -> Result<(), String> {
    let (host, port, path) = split_url(url).ok_or_else(|| format!("Not an https address: {url}"))?;
    let agent = wide(&format!("Azur-Files/{}", super::CURRENT));
    let host = wide(host);
    let verb = wide("GET");
    let path = wide(path);
    let headers = wide(&format!("Accept: {accept}\r\n"));

    // SAFETY: every string is null-terminated and outlives the call it is passed to; every handle
    // is checked before use and closed by `Handle`.
    unsafe {
        let session = Handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            return Err(failed("WinHttpOpen"));
        }
        // Resolve, connect, send, and then each read: long enough for a slow proxy, short enough
        // that a dead network gives up while somebody is still looking at the badge.
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 30_000);
        let connection = Handle(WinHttpConnect(session.0, host.as_ptr(), port, 0));
        if connection.0.is_null() {
            return Err(failed("WinHttpConnect"));
        }
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            verb.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(failed("WinHttpOpenRequest"));
        }
        // `u32::MAX` is `-1L`: the headers are null-terminated and WinHTTP measures them.
        if WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, null(), 0, 0, 0) == 0 {
            return Err(failed("WinHttpSendRequest"));
        }
        if WinHttpReceiveResponse(request.0, null_mut()) == 0 {
            return Err(failed("WinHttpReceiveResponse"));
        }
        match number(&request, WINHTTP_QUERY_STATUS_CODE) {
            Some(200) => {}
            Some(status) => return Err(format!("GitHub answered HTTP {status}")),
            None => return Err(failed("WinHttpQueryHeaders")),
        }
        let total = number(&request, WINHTTP_QUERY_CONTENT_LENGTH).map(u64::from);

        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            if WinHttpReadData(
                request.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
            ) == 0
            {
                return Err(failed("WinHttpReadData"));
            }
            if read == 0 {
                return Ok(());
            }
            sink(&buffer[..read as usize], total)?;
        }
    }
}

/// A header WinHTTP can give as a number: the status, the length.
fn number(request: &Handle, what: u32) -> Option<u32> {
    let mut value = 0u32;
    let mut len = std::mem::size_of::<u32>() as u32;
    // SAFETY: `value` is a `u32` and `len` says so.
    let ok = unsafe {
        WinHttpQueryHeaders(
            request.0,
            what | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut value as *mut u32).cast(),
            &mut len,
            null_mut(),
        )
    };
    (ok != 0).then_some(value)
}
