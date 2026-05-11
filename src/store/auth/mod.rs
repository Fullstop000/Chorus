//! Identity & auth tables: users, accounts, sessions, api_tokens.
//!
//! See `docs/plan/identity-and-auth-redesign.md` for the model. In short:
//! - `User` = the person. Stable identity, referenced everywhere as actor.
//! - `Account` = how a User authenticates. 1..N per User; `auth_provider`
//!   distinguishes local from cloud.
//! - `Session` = a browser cookie credential.
//! - `ApiToken` = a CLI or bridge bearer credential. Stored as SHA-256
//!   hash; the raw string is returned only at creation time.

pub mod accounts;
pub mod api_tokens;
pub mod sessions;
pub mod users;

pub use accounts::Account;
pub use api_tokens::{ApiToken, MintedToken};
pub use sessions::Session;
pub use users::User;
