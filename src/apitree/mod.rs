pub mod restapi;
pub mod wsapi;
use anyhow::{anyhow, Result};

pub fn ws(name: &str) -> Result<&'static wsapi::Api> {
    wsapi::WS_APIMAP
        .get(name)
        .ok_or_else(|| anyhow!("Exchange not supported"))
}

pub fn rest(name: &str) -> Result<restapi::Api> {
    restapi::REST_APIMAP
        .get(name)
        .ok_or_else(|| anyhow!("Exchange not supported"))
}
