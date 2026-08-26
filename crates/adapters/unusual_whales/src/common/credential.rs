// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

use std::{fmt::Debug, sync::Arc};

use zeroize::Zeroizing;

use super::consts::UNUSUAL_WHALES_API_TOKEN;

/// Shared Unusual Whales API credential.
#[derive(Clone)]
pub struct Credential {
    token: Arc<Zeroizing<String>>,
}

impl Credential {
    /// Resolves an explicit token or the documented environment variable.
    ///
    /// # Errors
    ///
    /// Returns an error when no non-empty token is available.
    pub fn resolve(explicit: Option<&str>) -> anyhow::Result<Self> {
        let token = explicit.map(str::to_owned).or_else(|| {
            std::env::var(UNUSUAL_WHALES_API_TOKEN)
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
        let token = token.ok_or_else(|| {
            anyhow::anyhow!("Unusual Whales API token is required; set {UNUSUAL_WHALES_API_TOKEN}")
        })?;
        anyhow::ensure!(
            token == token.trim(),
            "Unusual Whales API token has surrounding whitespace"
        );
        Ok(Self {
            token: Arc::new(Zeroizing::new(token)),
        })
    }

    /// Returns the credential for transport construction.
    #[must_use]
    pub fn token(&self) -> &str {
        self.token.as_str()
    }

    /// Returns the full account coordination scope hash without exposing the credential.
    #[must_use]
    pub fn scope_hash(&self, base_url: &str) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(base_url.as_bytes());
        hasher.update(&[0]);
        hasher.update(self.token.as_bytes());
        hasher.finalize().to_hex().to_string()
    }
}

impl Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(Credential))
            .field("token", &"<redacted>")
            .finish()
    }
}
