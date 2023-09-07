use crate::orderbook::{Orderbook, Side};
use anyhow::{anyhow, Result};
use bigdecimal::BigDecimal;
use futures_util::future::Future;
use log::info;
use serde::Deserialize;
use std::pin::Pin;
use std::str::FromStr;

pub struct Api {
    pub endpoint: &'static str,
    pub orderbook: Box<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Orderbook>>>>>,
}

pub struct Dummy {}

impl Dummy {
    pub fn get(&self, name: &str) -> Option<Api> {
        match name {
            "independentreserve" => Some(Api {
                endpoint: "https://api.independentreserve.com",
                orderbook: Box::new(|s| Box::pin(independentreserve_orderbook(s))),
            }),
            _ => None,
        }
    }
}

pub static REST_APIMAP: Dummy = Dummy {};

async fn independentreserve_orderbook(pair: String) -> Result<Orderbook> {
    let api = REST_APIMAP.get("independentreserve").unwrap();
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct Level {
        price: f64,
        volume: f64,
    }
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct OrderbookSnapshot {
        buy_orders: Vec<Level>,
        sell_orders: Vec<Level>,
    }
    let args: Vec<&str> = pair.split('-').collect();
    if args.len() != 2 {
        return Err(anyhow!(
            "pair in wrong format: should be Xbt-aud but got {}",
            pair
        ));
    }
    let endpoint = api.endpoint;
    let api = format!(
        "{}/Public/GetOrderbook?primaryCurrencyCode={}&secondaryCurrencyCode={}",
        endpoint, args[0], args[1]
    );
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
    let shot: OrderbookSnapshot = response.json().await.map_err(|e| anyhow!("{}", e))?;
    let mut ob = Orderbook::new("independentreserve");
    for level in shot.buy_orders {
        let price = BigDecimal::from_str(&format!("{}", level.price))
            .map_err(|e| anyhow!("parse price fail: {:?}", e))?;
        let v = BigDecimal::from_str(&format!("{}", level.volume))
            .map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
        ob.insert(Side::Bid, price, v);
    }
    for level in shot.sell_orders {
        let price = BigDecimal::from_str(&format!("{}", level.price))
            .map_err(|e| anyhow!("parse price fail: {:?}", e))?;
        let v = BigDecimal::from_str(&format!("{}", level.volume))
            .map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
        ob.insert(Side::Ask, price, v);
    }
    Ok(ob)
}
