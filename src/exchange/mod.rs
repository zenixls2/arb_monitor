use crate::apitree;
use crate::orderbook::Orderbook;
use actix_http::ws::Item::*;
use anyhow::{anyhow, Result};
use awc::ws::Frame::*;
use futures_util::{SinkExt, StreamExt};
use log::info;
use std::vec::Vec;

pub struct Exchange {
    name: String,
    pairs: Vec<String>,
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
            pairs: vec![],
            client,
            level: 10,
            connection: None,
            cache: "".to_string(),
        }
    }
    pub async fn connect(&mut self) -> Result<()> {
        let url = apitree::WS_APIMAP
            .get(&self.name)
            .ok_or_else(|| anyhow!("Exchange not supported"))?
            .endpoint;
        let (result, conn) = self
            .client
            .ws(url)
            .connect()
            .await
            .map_err(|e| anyhow!("{:?}", e))?;
        self.connection = Some(conn);
        info!("{:?}", result);
        Ok(())
    }
    pub async fn subscribe(&mut self, pair: &str) -> Result<()> {
        self.pairs.push(pair.to_string());
        if let Some(conn) = &mut self.connection {
            let request = apitree::WS_APIMAP
                .get(&self.name)
                .ok_or_else(|| anyhow!("Exchange not supported"))?
                .subscribe_text(pair, 20)?;
            info!("{:?}", request);
            conn.send(awc::ws::Message::Text(request.into()))
                .await
                .map(|_| ())
                .map_err(|e| anyhow!("{:?}", e))
        } else {
            Err(anyhow!("Not connect yet. Please run connect first."))
        }
    }
    pub async fn next(&mut self) -> Result<Option<Orderbook>> {
        if let Some(result) = self
            .connection
            .as_mut()
            .ok_or_else(|| anyhow!("Not connect yet. Please run connect first"))?
            .next()
            .await
        {
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
                Close(_) => return Err(anyhow!("close")),
            };

            let mut parsed = (apitree::WS_APIMAP
                .get(&self.name)
                .ok_or_else(|| anyhow!("Exchange not supported"))?
                .parse)(raw)?;
            parsed.trim(self.level);
            Ok(Some(parsed))
        } else {
            Ok(None)
        }
    }
}
