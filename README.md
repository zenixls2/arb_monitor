Arbitrage Monitor For Australia Exchanges
-----------------------------------------

The arbitrage monitor application right now supports the following exchanges:
- binance (orderbook only, websocket api only)
- bitstamp (orderbook only, webssocket api only)
- independentreserve (full functionality, respful api only)
- btcmarkets (full functionality, websocket api only)
- coinjar (full functionality, websocket api only)
- kraken (full functionality, websocket api only)

The project contains an example web page, `index.html`, that allows you to subscribe to the service.
Currently there's no authentication middleware protection for the websocket service.

#### Configuration Explanation

The example configuration stores in `config/config.yaml` in yaml format.

- `exchange_pair_map` object map:
    contains the exchange related configuration.

>> ```yaml
>> Format:
>> {exchange_name}:
>>   - pair: {pair_name}
>>     # independentreserve: {Token1}-{token2}
>>     # btcmarkets: {TOKEN1}-{TOKEN1}
>>     # conijar: {TOKEN1}{TOKEN2}
>>     # kraken: {TOKEN1}/{TOKEN2}
>>     # Notice XBT means BTC
>>   - ws_api: {bool}
>>     # (optional)
>>     # true for using websocket api, false for using restful api
>>     # currently only independentreserve supports restful api, the others support only websocket
>>   - wait_secs: {int}
>>     # (optional, functional when ws_api is false)
>>     # default value: 3
>>     # this sets the interval for polling orderbooks using restful api
>> ```

- `server_addr`:
  (optional) string.
  default value: 127.0.0.1
  currently not used. This is prepared for bot clients to connect.

- `bind_addr`:
  (optional) string
  default value: 0.0.0.0
  the websocket server binding address

- `server_port`:
  u16, default binds to 50051 port
  This is the port that the client should connect to

- `log_path`:
  (optional) string
  default: ./test.log
  The path where the log is stored.
  The log doesn't rotate. Please use logrotate to control the behavior.

- `log_level`:
  (optional) enum strings
  Options: "Error", "Warning", "Info", "Debug"
  Default: "Info"
  controls the log level of the service written to `log_path`

### Preparation

This project is construct usign rust. In order to run this project, you need to compile it to binaries first.

1. Install Rust
Any Unix-like OS should be able to install Rust. Windows could also install rust inside the WSL environment.
All you need to do is just install using the following command:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Don't forget to choose the nightly one from the list.
This project dependes on the nightly version of rust to provide some features.
after setup, try the following command to check the rust version to see if nightly version has been correctly installed:

```bash
rustc --version
```


2. Clone this repo

(Make sure you have access to the github, and have already imported your key to access private repo)

```bash
git clone git@github.com:zenixls2/arb_monitor.git
```

3. [Optional] Run test

Then run the unit tests in the project. Should have all tests passed.

```bash
cd arb_monitor && cargo test
```

4. Compile

Suppose you're already in the arb\_monitor folder, run the following command:

```bash
cargo build
```

and you can find the executable inside `target/debug/` folder with name `arb_monitor`.

5. [Optional] Compile the release binary

The release binary will be smaller, and apply several optimization to allow the program to run faster.

```bash
cargo build --release
```

The output binaries will be inside `target/release` instead.


### Execution

First check the usage by:

```bash
./target/debug/arb_monitor --help
```

And you could see how to use `arb_monitor`:

```bash
Usage: arb_monitor [OPTIONS]

Options:
  -c, --config-path <CONFIG_PATH>  [default: ./config/config.yaml]
  -h, --help                       Print help
  -V, --version                    Print version
```

If you're in the project root, simply run:

```bash
./target/debug/arb_monitor
```

and the service will start running.

The default configuration is stored in `config/config.yaml`. 

### Visualization

Please host the `index.html` on any http server in the same machine as the service.
Simply using python to host should also work:

```bash
# in project root
python3 -m http.server
```

and open `http://localhost:8000/` in browser.

Everytime when backend is restarted, we need to reload the page to re-connect.

### Deployment

Even though in the config you could set multiple different pairs, the output is unexpected.

We recommend users to set the same pair for all exchanges, and launch multiple services binding to different pairs on different ports. To switch pairs in the frontend side, simply connects to different ports. However, the frontend side needs some modification to support such feature.

Also if you have ufw or any other firewall services opened, don't forget to turn on the port.
For the example of ufw, should be:

```bash
# suppose the port used in config.yaml is 50051
sudo ufw allow 50051
# suppose we host the static html in nginx
sudo ufw allow "Nginx Full"
# well, this is optional
sudo ufw allow OpenSSH
```

#### Host using systemd

We could also use systemd to host the arb_monitor service. Just put the following service file to `/lib/systemd/system/btcaud.service`.

```ini
[Unit]
Description=BTC/AUD monitor
After=network.target nginx.service

[Service]
User=root
WorkingDirectory=/root
ExecStart=/root/arb_monitor -c ./btcaud.yaml
Restart=always
Type=simple
StandardOutput=syslog
KillMode=process

[Install]
WantedBy=multi-user.target
Alias=btcaud.service
```

We suppose our binary and config exist in the /root folder.

Then

```bash
systemctl daemon-reload
systemctl stop btcaud.service
systemctl start btcaud.service
```

To check the log:

```bash
journalctl -u btcaud.service
```

### File path in production server

- /var/www/html/index.html
- /var/www/html/usdcaud.mp3
- /var/www/html/usdtaud.mp3
- /var/www/html/btcaud.mp3
- /var/www/html/btcaud2.mp3
- /var/www/html/usdcaud2.mp3
- /var/www/html/usdtaud2.mp3
- /etc/nginx/.htpasswd
- /etc/nginx/sites-enabled/default
- /lib/systemd/system/btcaud.service
- /lib/systemd/system/usdtaud.service
- /lib/systemd/system/usdcaud.service
- /root/arb_monitor

To upgrade arb_monitor from the artifact.zip from github action, we need to stop these 3 services first:

```bash
systemctl stop btcaud.service
systemctl stop usdtaud.service
systemctl stop usdcaud.service
```

And upload the arb_monitor file to the root home directory:

```bash
# from host machine
scp arb_monitor root@{server_ip}:~/
```

The basic auth username and password is recorded in `/etc/nginx/.htpasswd`.

### Development

TBD
