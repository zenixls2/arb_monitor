use crate::apitree;
use crate::orderbook::Orderbook;
use actix_http::ws::Item::*;
use anyhow::{anyhow, Result};
use awc::ws::Frame::*;
use formatx::formatx;
use futures_util::{SinkExt, StreamExt};
use log::{debug, info};
use std::vec::Vec;

pub struct Exchange {
    name: String,
    client: awc::Client,
    level: u32,
    connection: Option<actix_codec::Framed<awc::BoxedSocket, awc::ws::Codec>>,
    cache: String,
}

impl Exchange {
    pub fn new(name: &str) -> Exchange {
        let client = awc::Client::builder()
            .max_http_version(awc::http::Version::HTTP_11)
            .finish();
        Exchange {
            name: name.to_string(),
            client,
            level: 10,
            connection: None,
            cache: "".to_string(),
        }
    }
    pub async fn connect(&mut self, pairs: Vec<String>) -> Result<()> {
        info!("start connect, {}", self.name);
        let api = apitree::WS_APIMAP
            .get(&self.name)
            .ok_or_else(|| anyhow!("Exchange not supported"))?;

        let mut url = api.endpoint.to_string();
        let render_url = api.render_url;
        if render_url {
            let p = pairs.join(",");

            info!("render Url: {}", p);
            url = formatx!(url, p).map_err(|e| anyhow!("{:?}", e))?;
        }
        info!("{}", url);

        let (result, mut conn) = self
            .client
            .ws(url)
            .connect()
            .await
            .map_err(|e| anyhow!("connection error: {:?}", e))?;
        info!("{:?}", result);
        if !render_url {
            for pair in pairs {
                let request = api.subscribe_text(&pair, 20)?;
                info!("{:?}", request);
                conn.send(awc::ws::Message::Text(request.into()))
                    .await
                    .map(|e| info!("{:?}", e))
                    .map_err(|e| anyhow!("{:?}", e))?;
            }
        }

        self.connection = Some(conn);
        Ok(())
    }
    pub async fn next(&mut self) -> Result<Option<Orderbook>> {
        let result = &mut self
            .connection
            .as_mut()
            .ok_or_else(|| anyhow!("Not connect yet. Please run connect first"))?;
        loop {
            if let Some(result) = result.next().await {
                let raw = match result? {
                    Text(msg) => std::str::from_utf8(&msg)?.to_string(),
                    Binary(msg) => std::str::from_utf8(&msg)?.to_string(),
                    Continuation(item) => match item {
                        FirstText(b) | FirstBinary(b) | Continue(b) => {
                            self.cache += std::str::from_utf8(&b)?;
                            return Ok(None);
                        }
                        Last(b) => {
                            let output = self.cache.clone() + std::str::from_utf8(&b)?;
                            self.cache = "".to_string();
                            output
                        }
                    },
                    Ping(_) | Pong(_) => return Ok(None),
                    Close(_) => return Err(anyhow!("close {}", self.name)),
                };

                debug!("{}: {}", self.name, raw);

                if let Some(mut e) = (apitree::WS_APIMAP
                    .get(&self.name)
                    .ok_or_else(|| anyhow!("Exchange not supported"))?
                    .parse)(raw)?
                {
                    e.trim(self.level);
                    return Ok(Some(e));
                }
                // skip none
            } else {
                return Ok(None);
            }
        }
    }
}
