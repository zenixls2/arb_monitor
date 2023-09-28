use crate::orderbook::{Orderbook, Side};
use anyhow::{anyhow, Result};
use bigdecimal::BigDecimal;
use futures_util::future::Future;
use log::info;
use phf::phf_map;
use serde::Deserialize;
use std::pin::Pin;
use std::str::FromStr;

type BoxFuture = Pin<Box<dyn Future<Output = Result<Orderbook>> + Send>>;

pub struct Api {
    pub endpoint: &'static str,
    pub orderbook: fn(String) -> BoxFuture,
}

pub static REST_APIMAP: phf::Map<&'static str, Api> = phf_map! {
    "independentreserve" => Api {
        endpoint: "https://api.independentreserve.com",
        orderbook: |s| Box::pin(independentreserve_orderbook(s)),
    },
    "btcmarkets" => Api {
        endpoint: "https://api.btcmarkets.net",
        orderbook: |s| Box::pin(btcmarkets_orderbook(s)),
    }
};

async fn btcmarkets_orderbook(pair: String) -> Result<Orderbook> {
    #[derive(Deserialize, Debug)]
    struct OrderbookSnapshot {
        asks: Vec<[String; 2]>,
        bids: Vec<[String; 2]>,
    }
    #[derive(Deserialize, Debug)]
    struct MarketSummary {
        volume24h: String,
        #[serde(rename = "lastPrice")]
        last_price: String,
    }
    let (ob_api, ticker_api) = {
        let rest = REST_APIMAP.get("btcmarkets").unwrap();
        let endpoint = rest.endpoint;
        (
            format!("{}/v3/markets/{}/orderbook", endpoint, pair),
            format!("{}/v3/markets/{}/ticker", endpoint, pair),
        )
    };
    info!("calling {}...", ob_api);
    let response = reqwest::get(&ob_api)
        .await
        .map_err(|e| anyhow!("{:?}", e))?;
    let shot: OrderbookSnapshot = response.json().await.map_err(|e| anyhow!("{}", e))?;

    info!("calling {}...", ticker_api);
    let response = reqwest::get(&ticker_api)
        .await
        .map_err(|e| anyhow!("{:?}", e))?;
    let sum: MarketSummary = response.json().await.map_err(|e| anyhow!("{}", e))?;
    let mut ob = Orderbook::new("btcmarkets");

    for [p, v] in shot.bids {
        let price = BigDecimal::from_str(&p).map_err(|e| anyhow!("parse price fail: {:?}", e))?;
        let volume = BigDecimal::from_str(&v).map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
        ob.insert(Side::Bid, price, volume);
    }
    for [p, v] in shot.asks {
        let price = BigDecimal::from_str(&p).map_err(|e| anyhow!("parse price fail: {:?}", e))?;
        let volume = BigDecimal::from_str(&v).map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
        ob.insert(Side::Ask, price, volume);
    }
    ob.last_price = BigDecimal::from_str(&sum.last_price)
        .map_err(|e| anyhow!("parse last_price fail: {:?}", e))?;
    ob.volume =
        BigDecimal::from_str(&sum.volume24h).map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
    Ok(ob)
}

async fn independentreserve_orderbook(pair: String) -> Result<Orderbook> {
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
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct MarketSummary {
        last_price: f64,
        day_volume_xbt: f64,
    }
    let args: Vec<&str> = pair.split('-').collect();
    if args.len() != 2 {
        return Err(anyhow!(
            "pair in wrong format: should be Xbt-aud but got {}",
            pair
        ));
    }
    let (ob_api, sum_api) = {
        let api = REST_APIMAP.get("independentreserve").unwrap();
        let endpoint = api.endpoint;
        (
            format!(
                "{}/Public/GetOrderbook?primaryCurrencyCode={}&secondaryCurrencyCode={}",
                endpoint, args[0], args[1]
            ),
            format!(
                "{}/Public/GetMarketSummary?primaryCurrencyCode={}&secondaryCurrencyCode={}",
                endpoint, args[0], args[1]
            ),
        )
    };
    info!("calling {}...", ob_api);
    let response = reqwest::get(&ob_api)
        .await
        .map_err(|e| anyhow!("{:?}", e))?;
    let shot: OrderbookSnapshot = response.json().await.map_err(|e| anyhow!("{}", e))?;

    info!("calling {}...", sum_api);
    let response = reqwest::get(&sum_api)
        .await
        .map_err(|e| anyhow!("{:?}", e))?;
    let sum: MarketSummary = response.json().await.map_err(|e| anyhow!("{}", e))?;
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
    ob.last_price = BigDecimal::from_str(&format!("{}", sum.last_price))
        .map_err(|e| anyhow!("parse last_price fail: {:?}", e))?;
    ob.volume = BigDecimal::from_str(&format!("{}", sum.day_volume_xbt))
        .map_err(|e| anyhow!("parse volume fail: {:?}", e))?;
    Ok(ob)
}
