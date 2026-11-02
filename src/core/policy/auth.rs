//! GitHub credential environment variables.

/// Environment variables holding the GitHub token, in lookup order. The first non-empty value wins.
pub(crate) const TOKEN_ENV_VARS: [&str; 2] = ["GITHUB_TOKEN", "GH_TOKEN"];
