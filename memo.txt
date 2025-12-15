## 起動方法

$env:CONFIG="config.toml"
cargo run --release

## 初回の手順

- config.example.toml を config.toml にコピー
- bind と redirect_url のポートを適宜変える
- https://dev.twitch.tv/console/apps で新規作成
  - リダイレクトURL を redirect_url と揃える (127.0.0.1 ではなく localhost にする)
  - カテゴリーは Application Integration，クライアントのタイプは機密保持にする。
  - secret を config.toml に書く
- ログインする
- target_reward_id を調べて設定する

