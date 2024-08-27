# zed-code-stats

A [Wakatime](https://wakatime.com/) extension for [Zed](https://zed.dev/).

Uses the [wakatime-ls](https://github.com/bestgopher/wakatime-zed/wakatime-ls) to receive edit events from Zed and send hearbeat to Wakatime by [wakatime-cli](https://github.com/wakatime/wakatime-cli).

## Configuration
In order to authenticate with the Wakatime-cli, the language server needs to know your API token.
Here are two ways to configurate the lsp.

### Wakatime configuration file
1. create a file named `.wakatime.cfg`, locate your HOME directory.
```toml
[settings]
api_key = Your api key
```
2. Zed setting.Open zed setting file, add your api key
```toml
"lsp": {
  "wakatime": {
    "settings": {
      "api-key": "You api key"
    }
  }
}
```
