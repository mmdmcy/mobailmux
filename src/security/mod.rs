//! Authentication and public-source trust boundaries.

mod audit;
mod auth;

pub(crate) use audit::audit_public_cmd;
#[cfg(test)]
pub(crate) use audit::{audit_path, contains_tailscale_ipv4, suspicious_secret_assignment};
pub(crate) use auth::{
    hash_password_cmd, login_page, login_post, logout_post, page_guard, random_secret, raw_guard,
};
#[cfg(test)]
pub(crate) use auth::{password_digest, verify_password};
