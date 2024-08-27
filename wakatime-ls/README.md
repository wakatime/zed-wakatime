# code-stats-ls

A language server for [Code::Stats](https://codestats.net/).

## Installation

See the [releases](https://github.com/maxdeviant/code-stats-ls/releases) page for pre-built binaries.

If you would like to install the Code::Stats language server from source you can run:

```sh
git clone git@github.com:maxdeviant/code-stats-ls.git
cd code-stats-ls
cargo install --path .
```

## Authentication

In order to authenticate with the Code::Stats API, the language server needs to know your API token.

Authentication is handled by environment variables.

You may either add the `CODE_STATS_API_TOKEN` environment variable to your shell, or create a `~/.env` file:

```
CODE_STATS_API_TOKEN=<API_TOKEN>
```