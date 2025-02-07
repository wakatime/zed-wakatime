# zed-wakatime
![Dynamic JSON Badge](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fapi.zed.dev%2Fextensions%2Fwakatime&query=%24.data%5B0%5D.download_count&label=download&cacheSeconds=60)

A [WakaTime](https://wakatime.com/) extension for [Zed](https://zed.dev/).

Uses the [wakatime-ls](https://github.com/wakatime/zed-wakatime/tree/master/wakatime-ls) to receive edit events from Zed and send hearbeats to WakaTime by [wakatime-cli](https://github.com/wakatime/wakatime-cli).

## Install
Search "wakatime" in extension page, and install it.
![type install](./images/install.png)

## Configuration
In order to authenticate with the wakatime-cli, the language server needs to know your API token.
Here are two ways to set the lsp.

### WakaTime configuration file
create a file named `.wakatime.cfg`, locate your HOME directory.
```toml
[settings]
api_key = Your api key
```

### zed setting file
Zed setting.Open zed setting file, add your api key
```json
"lsp": {
  "wakatime": {
    "settings": {
      "api-key": "You api key"
    }
  }
}
```

## Note
This plugin has been thoroughly tested only on macOS. If you encounter any issues on other systems, please submit an issue or a pull request.
