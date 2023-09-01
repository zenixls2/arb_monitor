use crate::orderbook::{Orderbook, Side};
use anyhow::{anyhow, Result};
use bigdecimal::BigDecimal;
use formatx::formatx;
use once_cell::sync::Lazy;
use phf::phf_map;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

type ParseFunc = fn(String) -> Result<Option<Orderbook>>;
#[derive(Clone)]
pub struct Api {
    pub endpoint: &'static str,
    // (pair, level)
    pub subscribe_template: &'static str,
    // raw String as input
    pub parse: ParseFunc,
    // render url with data
    pub render_url: bool,
}

impl Api {
    // utility to render the subscription text
    pub fn subscribe_text(&self, pair: &str, level: u32) -> Result<String> {
        formatx!(self.subscribe_template, pair, level).map_err(|e| anyhow!("{:?}", e))
    }
}

fn binance_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Default, Deserialize, Debug)]
    #[serde(rename_all = "camelCase", default)]
    struct PartialBookDepth {
        last_update_id: u64,
        bids: Vec<[String; 2]>,
        asks: Vec<[String; 2]>,
        result: Value,
        id: u64,
    }
    // PartialBookDepth is the only subscription type
    // others should be categorized as error
    let result: PartialBookDepth = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    // this is a subscription response
    if result.last_update_id == 0 && result.bids.is_empty() && result.asks.is_empty() {
        return Ok(None);
    }
    if result.result != Value::Null {
        return Err(anyhow!("result not empty"));
    }

    let mut ob = Orderbook::new("binance");
    for [price_str, quantity_str] in result.bids {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Bid, price, quantity);
    }
    for [price_str, quantity_str] in result.asks {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Ask, price, quantity);
    }
    Ok(Some(ob))
}

fn bitstamp_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Deserialize, Debug)]
    struct LiveDetailOrderbook {
        bids: Vec<[String; 2]>,
        asks: Vec<[String; 2]>,
        #[serde(rename = "timestamp")]
        _timestamp: String,
        #[serde(rename = "microtimestamp")]
        _microtimestamp: String,
    }
    #[derive(Deserialize, Debug)]
    struct WsEvent {
        data: Value,
        event: String,
        channel: String,
    }
    let result: WsEvent = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    if result.event != "data" {
        // return an empty Orderbook. This might be a response or reconnect request
        // we'll ignore reconnection handling at this moment
        return Ok(None);
    }
    if !result.channel.starts_with("order_book_") {
        return Err(anyhow!("non-orderbook signal passed it"));
    }
    // LiveDetailOrderbook is the only subscription type
    // others should be categorized as error
    let result: LiveDetailOrderbook =
        serde_json::from_value(result.data).map_err(|e| anyhow!("{:?}", e))?;
    let mut ob = Orderbook::new("bitstamp");
    for [price_str, quantity_str] in result.bids {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Bid, price, quantity);
    }
    for [price_str, quantity_str] in result.asks {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Ask, price, quantity);
    }
    Ok(Some(ob))
}

static INDRESERVE: Lazy<Mutex<HashMap<String, Orderbook>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn indreserve_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct Unit {
        price: f64,
        volume: f64,
    }
    #[derive(Deserialize, Debug)]
    struct Snapshot {
        #[serde(rename = "Bids")]
        bids: Vec<Unit>,
        #[serde(rename = "Offers")]
        asks: Vec<Unit>,
        #[serde(rename = "Crc32")]
        _crc32: u64,
    }
    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "PascalCase")]
    struct WsEvent {
        #[serde(default)]
        channel: String,
        #[serde(default)]
        data: Value,
        event: String,
    }
    let result: WsEvent = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    if result.event == "Subscriptions" {
        let mut tmp = INDRESERVE.lock().unwrap();
        let result: Vec<String> =
            serde_json::from_value(result.data).map_err(|e| anyhow!("{:?}", e))?;
        for channel in result {
            tmp.insert(channel, Orderbook::new("independentreserve"));
        }
        return Ok(None);
    } else if result.event != "OrderBookSnapshot" && result.event != "OrderBookChange" {
        return Ok(None);
    }
    let mut tmp = INDRESERVE.lock().unwrap();
    if let Some(ob) = tmp.get_mut(&result.channel) {
        let result: Snapshot =
            serde_json::from_value(result.data).map_err(|e| anyhow!("{:?}", e))?;
        for Unit { price, volume } in result.bids {
            let p = BigDecimal::from_str(&format!("{}", price))
                .map_err(|e| anyhow!("parse price fail: {} {:?}", price, e))?;
            let v = BigDecimal::from_str(&format!("{}", volume))
                .map_err(|e| anyhow!("parse volume fail: {} {:?}", volume, e))?;
            ob.insert(Side::Bid, p, v);
        }
        for Unit { price, volume } in result.asks {
            let p = BigDecimal::from_str(&format!("{}", price))
                .map_err(|e| anyhow!("parse price fail: {} {:?}", price, e))?;
            let v = BigDecimal::from_str(&format!("{}", volume))
                .map_err(|e| anyhow!("parse volume fail: {} {:?}", volume, e))?;
            ob.insert(Side::Ask, p, v);
        }
        Ok(Some(ob.clone()))
    } else {
        Err(anyhow!("orderbook not exist for {}", result.channel))
    }
}

fn btcmarkets_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Deserialize, Debug)]
    struct WsEvent {
        bids: Vec<[String; 2]>,
        asks: Vec<[String; 2]>,
        #[serde(rename = "messageType")]
        message_type: String,
    }
    let result: WsEvent = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    if result.message_type != "orderbook" {
        return Ok(None);
    }
    let mut ob = Orderbook::new("btcmarkets");
    for [price_str, quantity_str] in result.bids {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Bid, price, quantity);
    }
    for [price_str, quantity_str] in result.asks {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Ask, price, quantity);
    }
    Ok(Some(ob))
}

static COINJAR: Lazy<Mutex<HashMap<String, Orderbook>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn coinjar_parser(raw: String) -> Result<Option<Orderbook>> {
    #[derive(Deserialize, Debug)]
    struct Payload {
        #[serde(default)]
        bids: Vec<[String; 2]>,
        #[serde(default)]
        asks: Vec<[String; 2]>,
    }

    #[derive(Deserialize, Debug)]
    struct WsEvent {
        event: String,
        payload: Payload,
        topic: String,
    }
    let result: WsEvent = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    if result.event != "init" && result.event != "update" {
        return Ok(None);
    }
    let key = &result.topic;
    let mut tmp = COINJAR.lock().unwrap();
    let ob = if let Some(ob) = tmp.get_mut(key) {
        ob
    } else {
        tmp.insert(key.clone(), Orderbook::new("coinjar"));
        tmp.get_mut(key).unwrap()
    };
    let result = result.payload;
    for [price_str, quantity_str] in result.bids {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Bid, price, quantity);
    }
    for [price_str, quantity_str] in result.asks {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Ask, price, quantity);
    }
    Ok(Some(ob.clone()))
}

static KRAKEN: Lazy<Mutex<HashMap<String, Orderbook>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn kraken_parser(raw: String) -> Result<Option<Orderbook>> {
    if raw.as_bytes()[0] as char == '{' {
        return Ok(None);
    }
    #[derive(Deserialize, Debug)]
    struct Data {
        #[serde(default)]
        r#as: Vec<[String; 3]>,
        #[serde(default)]
        bs: Vec<[String; 3]>,
        #[serde(default)]
        a: Vec<Vec<String>>,
        #[serde(default)]
        b: Vec<Vec<String>>,
    }
    let result: Vec<Value> = serde_json::from_str(&raw).map_err(|e| anyhow!("{:?}", e))?;
    let channel_name: String =
        serde_json::from_value(result[2].clone()).map_err(|e| anyhow!("{:?}", e))?;
    let pair: String = serde_json::from_value(result[3].clone()).map_err(|e| anyhow!("{:?}", e))?;
    let key = channel_name + &pair;
    // channel_id: u64
    // data: object
    // - as: Vec<[String; 3]>
    // - bs: Vec<[String; 3]>
    // channel_name: String
    // pair: String
    let data: Data = serde_json::from_value(result[1].clone()).map_err(|e| anyhow!("{:?}", e))?;
    let mut tmp = KRAKEN.lock().unwrap();
    let ob = if let Some(ob) = tmp.get_mut(&key) {
        ob
    } else {
        tmp.insert(key.clone(), Orderbook::new("kraken"));
        tmp.get_mut(&key).unwrap()
    };
    for [price_str, quantity_str, _timestamp] in data.bs {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Bid, price, quantity);
    }
    for v in data.b {
        let price_str: &str = &v[0];
        let quantity_str: &str = &v[1];
        let price = BigDecimal::from_str(price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Bid, price, quantity);
    }
    for [price_str, quantity_str, _timestamp] in data.r#as {
        let price = BigDecimal::from_str(&price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(&quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Ask, price, quantity);
    }
    for v in data.a {
        let price_str: &str = &v[0];
        let quantity_str: &str = &v[1];
        let price = BigDecimal::from_str(price_str).map_err(|e| anyhow!("{:?}", e))?;
        let quantity = BigDecimal::from_str(quantity_str).map_err(|e| anyhow!("{:?}", e))?;
        ob.insert(Side::Ask, price, quantity);
    }
    Ok(Some(ob.clone()))
}

// The API Map compile-time static map that handles depth orderbook subscription and parsing
pub static WS_APIMAP: phf::Map<&'static str, Api> = phf_map! {
    "binance" => Api {
        endpoint: "wss://stream.binance.com:9443/ws",
        subscribe_template: r#"{{"id": 1, "method": "SUBSCRIBE", "params": ["{}@depth{}@100ms"]}}"#,
        parse: (binance_parser as ParseFunc),
        render_url: false,
    },
    "binance_futures" => Api {
        endpoint: "wss://fstream.binance.com:9443/ws",
        subscribe_template: r#"{{"id":1, "method":"SUBSCRIBE", "params": ["{}@depth{}@100ms"]}}"#,
        parse: (binance_parser as ParseFunc),
        render_url: false,
    },
    "bitstamp" => Api {
        endpoint: "wss://ws.bitstamp.net",
        subscribe_template: r#"{{"event":"bts:subscribe","data":{{"channel":"order_book_{}"}}}}"#,
        parse: (bitstamp_parser as ParseFunc),
        render_url: false,
    },
    "independentreserve" => Api {
        endpoint: "wss://websockets.independentreserve.com/orderbook/20?subscribe={}",
        subscribe_template: r#"{{"Event": "Subscribe", "Data": ["{}"]}}"#,
        parse: (indreserve_parser as ParseFunc),
        render_url: true,
    },
    "btcmarkets" => Api {
        endpoint: "wss://socket.btcmarkets.net/v2",
        subscribe_template: r#"{{"marketIds": ["{}"], "channels": ["orderbook"], "messageType": "subscribe"}}"#,
        parse: (btcmarkets_parser as ParseFunc),
        render_url: false,
    },
    "coinjar" => Api {
        endpoint: "wss://feed.exchange.coinjar.com/socket/websocket",
        subscribe_template: r#"{{"topic": "book:{}", "event": "phx_join", "payload": {{}}, "ref": 0}}"#,
        parse: (coinjar_parser as ParseFunc),
        render_url: false,
    },
    "kraken" => Api {
        endpoint: "wss://ws.kraken.com",
        subscribe_template: r#"{{"event":"subscribe","pair":["{}"], "subscription": {{"name":"book","depth":25}}}}"#,
        parse: (kraken_parser as ParseFunc),
        render_url: false,
    }
};

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use std::str::FromStr;
    #[test]
    fn test_subscribe_text() {
        let rendered = super::WS_APIMAP
            .get("binance")
            .unwrap()
            .subscribe_text("BTCUSDT", 20)
            .unwrap();
        assert_eq!(
            rendered,
            r#"{"id": 1, "method": "SUBSCRIBE", "params": ["BTCUSDT@depth20@100ms"]}"#
        );
    }
    #[test]
    fn test_binance_parse() {
        // subscription response, return empty Orderbook
        let out = (super::WS_APIMAP.get("binance").unwrap().parse)(
            r#"{"id": 1, "result": null}"#.to_string(),
        )
        .unwrap();
        assert_eq!(out, None);

        // normal event
        let out = (super::WS_APIMAP.get("binance").unwrap().parse)(
            r#"{"lastUpdateId": 160, "bids":[["0.01", "0.2"]], "asks": []}"#.to_string(),
        )
        .unwrap();
        let mut ob = super::Orderbook::new("binance");
        ob.insert(
            super::Side::Bid,
            BigDecimal::from_str("0.01").unwrap(),
            BigDecimal::from_str("0.2").unwrap(),
        );
        assert_eq!(out, Some(ob));
    }
    #[test]
    fn test_bitstamp_parse() {
        // subscription response
        let out = (super::WS_APIMAP.get("bitstamp").unwrap().parse)(
            r#"{"event": "bts:subscription_succeeded", "channel": "order_book_btcusd", "data": {}}"#
                .to_string(),
        )
        .unwrap();
        assert_eq!(out, None);

        // normal event
        let out = (super::WS_APIMAP.get("bitstamp").unwrap().parse)(
            r#"{"data":{
                "timestamp":"1691595437",
                "microtimestamp":"1691595437334962",
                "bids":[],
                "asks":[["29737","0.67548438"],["29738","0.67255217"]]
            },"channel":"order_book_btcusd","event":"data"}"#
                .to_string(),
        )
        .unwrap();
        let mut ob = super::Orderbook::new("bitstamp");
        ob.insert(
            super::Side::Ask,
            BigDecimal::from_str("29737").unwrap(),
            BigDecimal::from_str("0.67548438").unwrap(),
        );
        ob.insert(
            super::Side::Ask,
            BigDecimal::from_str("29738").unwrap(),
            BigDecimal::from_str("0.67255217").unwrap(),
        );
        assert_eq!(out, Some(ob));
    }
    #[test]
    fn test_indreserve_parse() {
        // subscription response
        (super::WS_APIMAP.get("independentreserve").unwrap().parse)(
            r#"{"Data": ["orderbook/5/btc/aud"], "Event": "Subscriptions", "Time": 1660895883834}"#
                .to_string(),
        )
        .unwrap();
        let out = (super::WS_APIMAP.get("independentreserve").unwrap().parse)(
            r#"{"Channel": "orderbook/5/btc/aud","Data": {
                "Bids": [{
                    "Price": 31802.46,"Volume": 0.25
                },{
                    "Price": 31802.45,"Volume": 0.32464684
                }],
                "Offers": [{
                    "Price": 31844.99,"Volume": 0.30740328
                },{
                    "Price": 31845,"Volume": 1.5
                }],
                "Crc32": 2893776693
              },
              "Time": 1660895883834,"Event": "OrderBookSnapshot"
            }"#
            .to_string(),
        )
        .unwrap();
        let mut ob = super::Orderbook::new("independentreserve");
        ob.insert(
            super::Side::Bid,
            BigDecimal::from_str("31802.46").unwrap(),
            BigDecimal::from_str("0.25").unwrap(),
        );
        ob.insert(
            super::Side::Bid,
            BigDecimal::from_str("31802.45").unwrap(),
            BigDecimal::from_str("0.32464684").unwrap(),
        );
        ob.insert(
            super::Side::Ask,
            BigDecimal::from_str("31844.99").unwrap(),
            BigDecimal::from_str("0.30740328").unwrap(),
        );
        ob.insert(
            super::Side::Ask,
            BigDecimal::from_str("31845").unwrap(),
            BigDecimal::from_str("1.5").unwrap(),
        );
        assert_eq!(out, Some(ob));
    }
}
