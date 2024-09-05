# wakatime-zed

A [Wakatime](https://wakatime.com/) extension for [Zed](https://zed.dev/).

Uses the [wakatime-ls](https://github.com/bestgopher/wakatime-zed/tree/master/wakatime-ls) to receive edit events from Zed and send hearbeats to Wakatime by [wakatime-cli](https://github.com/wakatime/wakatime-cli).

## Install
Search "wakatime" in extension page, and install it.
![type install](./images/install.png)

## Configuration
In order to authenticate with the Wakatime-cli, the language server needs to know your API token.
Here are two ways to set the lsp.

### Wakatime configuration file
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
