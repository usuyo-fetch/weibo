//! `canonicalize`：认出微博的身份，不发请求（链接里已经有 `mid` 或 `mblogid`）。`fetch` 用同一个函数，所以两边给的身份一样。

use usuyo_plugin_kit::protocol::{CallFailure, Canonicalized, ErrorCode, Identity, PlatformId};

use crate::ids::{Parsed, canonical_url, parse_input};

pub fn identity(mid: &str) -> Result<Identity, CallFailure> {
    let platform =
        PlatformId::try_from("weibo").map_err(|e| CallFailure::new(ErrorCode::Internal, e))?;
    Ok(Identity::Platform {
        platform,
        content_id: mid.to_owned(),
        url: Some(canonical_url(mid)),
    })
}

pub fn canonicalize(input: &str) -> Result<Canonicalized, CallFailure> {
    Ok(match parse_input(input) {
        Parsed::Post { mid } => Canonicalized::Match {
            identity: identity(&mid)?,
            resolved_url: None,
        },
        Parsed::Foreign => Canonicalized::NoMatch,
    })
}
