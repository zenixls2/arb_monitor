use crate::apitree;
use crate::config::ExchangeSetting;
use crate::orderbook::Orderbook;
use actix_http::ws::Item::*;
use anyhow::{anyhow, Result};
use awc::ws::Frame::*;
use formatx::formatx;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info};
use std::vec::Vec;
use tokio::time::{sleep, Duration, Instant};

pub struct Exchange {
    name: String,
    client: awc::Client,
    level: u32,
    connection: Option<actix_codec::Framed<awc::BoxedSocket, awc::ws::Codec>>,
    cache: String,
    ws_api: bool,
    pairs: Vec<String>,
    wait_secs: u64,
    heartbeat_ts: Option<Instant>,
    reconnect_ts: Option<Instant>,
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
            ws_api: true,
            pairs: vec![],
            wait_secs: 0,
            heartbeat_ts: None,
            reconnect_ts: None,
        }
    }
    pub async fn connect(&mut self, pairs: Vec<ExchangeSetting>) -> Result<()> {
        self.pairs = pairs.iter().map(|e| e.pair.clone()).collect();
        let default_setup = pairs
            .get(0)
            .ok_or_else(|| anyhow!("should have at least one pair setting"))?;
        // wait_secs here is only used in rest api
        self.wait_secs = if default_setup.wait_secs > 0 {
            default_setup.wait_secs
        } else {
            1_u64
        };
        self.ws_api = default_setup.ws_api;
        if !self.ws_api {
            return Ok(());
        }
        info!("start connect, {}", self.name);
        let api = apitree::ws(&self.name)?;

        let mut url = api.endpoint.to_string();
        let render_url = api.render_url;
        if render_url {
            let p = self.pairs.join(",");

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
            for pair in self.pairs.iter() {
                let requests = api.subscribe_text(pair, 20)?;
                info!("{:?}", requests);
                for request in requests {
                    conn.send(awc::ws::Message::Text(request.into()))
                        .await
                        .map(|e| info!("{:?}", e))
                        .map_err(|e| anyhow!("{:?}", e))?;
                }
            }
        }

        self.connection = Some(conn);
        Ok(())
    }
    pub fn clear(&self) -> Result<()> {
        let api = apitree::ws(&self.name)?;
        (api.clear)();
        Ok(())
    }
    pub async fn next(&mut self) -> Result<Option<Orderbook>> {
        if !self.ws_api {
            let level = self.level;
            sleep(Duration::from_secs(self.wait_secs)).await;
            // only able to handle one pair
            if let Some(pair) = self.pairs.first() {
                return (apitree::rest(&self.name)?.orderbook)(pair.clone())
                    .await
                    .map(move |mut e| {
                        e.trim(level);
                        Some(e)
                    });
            }
            return Err(anyhow!("no pair assigned to the exchange"));
        }
        let result = &mut self
            .connection
            .as_mut()
            .ok_or_else(|| anyhow!("Not connect yet. Please run connect first"))?;
        let api = apitree::ws(&self.name)?;
        let (wait_secs, msg) = api.heartbeat.unwrap_or((0, ""));
        let reconn_secs = api.reconnect_sec.unwrap_or(0);
        info!("reconn_secs: {}", reconn_secs);
        if self.heartbeat_ts.is_none() && wait_secs > 0 {
            self.heartbeat_ts = Some(Instant::now());
        }
        if self.reconnect_ts.is_none() && reconn_secs > 0 {
            self.reconnect_ts = Some(Instant::now());
        }
        loop {
            // sending heartbeats
            if let Some(now) = self.heartbeat_ts {
                if wait_secs < now.elapsed().as_secs() {
                    info!("send heartbeat to {}", self.name);
                    self.heartbeat_ts = Some(Instant::now());
                    if let Err(e) = result
                        .send(awc::ws::Message::Binary(msg.into()))
                        .await
                        .map(|e| info!("{:?}", e))
                    {
                        error!("heartbeat: {}", e);
                    }
                }
            }
            if let Some(now) = self.reconnect_ts {
                if reconn_secs < now.elapsed().as_secs() {
                    // force close the connection
                    info!("reconnect: {}", self.name);
                    return Err(anyhow!("close {}", self.name));
                }
            }
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
                    Close(_) => {
                        error!("stream gets closed: {}", self.name);
                        return Err(anyhow!("close {}", self.name));
                    }
                };

                debug!("{}: {}", self.name, raw);

                if let Some(mut e) = (apitree::ws(&self.name)?.parse)(&raw)
                    .map_err(|e| anyhow!("{}: raw msg: {}", e, raw))?
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
