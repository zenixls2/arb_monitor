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
            "btcmarkets" => Some(Api {
                endpoint: "https://api.btcmarkets.net",
                orderbook: Box::new(|s| Box::pin(btcmarkets_orderbook(s))),
            }),
            "coinspot" => Some(Api {
                endpoint: "https://www.coinspot.com.au",
                orderbook: Box::new(|s| Box::pin(coinspot_orderbook(s))),
            }),
            _ => None,
        }
    }
}

pub static REST_APIMAP: Dummy = Dummy {};

async fn coinspot_orderbook(pair: String) -> Result<Orderbook> {
    let api = REST_APIMAP.get("coinspot").unwrap();
    let endpoint = api.endpoint;
    let api = format!("{}/pubapi/v2/orders/open/{}", endpoint, pair);
    info!("calling {}...", api);

    #[derive(Deserialize, Debug)]
    struct Level {
        amount: f64, // amount that was bought
        rate: f64,   // latest buy/sell price for that coin
    }
    #[derive(Deserialize, Debug)]
    struct OpenMarketOrders {
        status: String,
        message: String,
        buyorders: Vec<Level>,
        sellorders: Vec<Level>,
    }
    let response = reqwest::get(&api).await?;
    let orders: OpenMarketOrders = response.json().await?;
    info!("{:?}", orders);
    if orders.status != "ok" {
        return Err(anyhow!("{}: {}", orders.status, orders.message));
    }

    let mut ob = Orderbook::new("coinspot");

    for lvl in orders.buyorders {
        let price = BigDecimal::from_str(&format!("{}", lvl.rate))
            .map_err(|e| anyhow!("parse price fail: {}", e))?;
        let volume = BigDecimal::from_str(&format!("{}", lvl.amount))
            .map_err(|e| anyhow!("volume price fail: {}", e))?;
        ob.insert(Side::Bid, price, volume);
    }
    for lvl in orders.sellorders {
        let price = BigDecimal::from_str(&format!("{}", lvl.rate))
            .map_err(|e| anyhow!("parse price fail: {}", e))?;
        let volume = BigDecimal::from_str(&format!("{}", lvl.amount))
            .map_err(|e| anyhow!("parse volume fail: {}", e))?;
        ob.insert(Side::Ask, price, volume);
    }
    #[derive(Deserialize, Debug)]
    struct Price {
        last: String,
    }
    #[derive(Deserialize, Debug)]
    struct LatestPrice {
        status: String,
        #[serde(default)]
        message: String,
        prices: Price,
    }

    let api = format!("{}/pubapi/v2/latest/{}", endpoint, pair);
    info!("calling {}...", api);
    let response = reqwest::get(&api).await?;
    let last_price: LatestPrice = response.json().await?;
    if last_price.status != "ok" {
        return Err(anyhow!("{}: {}", last_price.status, last_price.message));
    }
    ob.last_price = BigDecimal::from_str(&last_price.prices.last)
        .map_err(|e| anyhow!("parse last price fail {}", e))?;

    // since coinspot doesn't have any volume 24h information exposed,
    // the volume will be always 0.
    Ok(ob)
}

async fn btcmarkets_orderbook(pair: String) -> Result<Orderbook> {
    let api = REST_APIMAP.get("btcmarkets").unwrap();
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
    let endpoint = api.endpoint;
    let api = format!("{}/v3/markets/{}/orderbook", endpoint, pair);
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
    let shot: OrderbookSnapshot = response.json().await.map_err(|e| anyhow!("{}", e))?;

    let api = format!("{}/v3/markets/{}/ticker", endpoint, pair);
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
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
    let endpoint = api.endpoint;
    let api = format!(
        "{}/Public/GetOrderbook?primaryCurrencyCode={}&secondaryCurrencyCode={}",
        endpoint, args[0], args[1]
    );
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
    let shot: OrderbookSnapshot = response.json().await.map_err(|e| anyhow!("{}", e))?;

    let api = format!(
        "{}/Public/GetMarketSummary?primaryCurrencyCode={}&secondaryCurrencyCode={}",
        endpoint, args[0], args[1]
    );
    info!("calling {}...", api);
    let response = reqwest::get(&api).await.map_err(|e| anyhow!("{:?}", e))?;
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
