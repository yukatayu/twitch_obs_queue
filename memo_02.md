# twitch_obs_queue (Rust)

Twitch の「チャンネルポイント報酬」(特定の報酬ID)が交換されたら、
そのユーザーを **キューに追加**し、OBS Browser Source で一覧表示します。

- 24時間以内（設定可能）に「参加完了」した回数が少ない人ほど前に入ります
- 既にキューにいる人が再度交換しても無視
- SQLite に保存するので再起動してもキュー維持
- 管理画面(ブラウザ)で、完了/キャンセルで削除・並べ替え
- ユーザーアイコン(URL)はDBにキャッシュして、Helix 呼び出しを減らします

開発中に再起動を繰り返すと EventSub の WebSocket サブスクリプションが溜まりがちなので、
本アプリは **切断済み(非enabled)のWebSocketサブスクリプションをベストエフォートで掃除**します。

## 使い方（最短ルート）

1. Twitch Developer Console でアプリ作成
   - Redirect URL に `http://localhost:3000/auth/callback` を登録（ポートは config.toml の `server.bind` と合わせる）
2. `config.example.toml` を `config.toml` にコピー
3. `config.toml` に `twitch.client_id` / `twitch.client_secret` を入れる
4. 起動
   ```bash
   CONFIG=config.toml cargo run --release
   ```
5. ブラウザで管理画面を開く
   - `http://localhost:3000/admin`
   - 「Twitchでログイン」
6. 報酬IDを確認
   - 右上の「報酬ID一覧」から、使いたい報酬の `id` をコピー
   - `config.toml` の `twitch.target_reward_id` に貼り付けて再起動
7. OBS に追加
   - Browser Source URL: `http://127.0.0.1:3000/obs`

## 管理操作

- ✅完了: キューから削除し、「参加完了」として履歴(参与回数)に記録
- ❌キャンセル: キューから削除するが、「参加完了」には数えない
- ↑/↓: 並べ替え

※ 「完了/キャンセル」の区別は **優先度計算(過去window内の参加回数)** に影響します。

## 設定 (config.toml)

- `queue.participation_window_secs`
  - 24h を変えたい場合はここ
- `queue.processed_message_ttl_secs`
  - EventSub の `message_id` をどれくらい保持して重複通知を弾くか

- `twitch.user_cache_ttl_secs`
  - ユーザーのアイコンURLなどをDBにキャッシュする期間（秒）
  - 0 にすると毎回Helixから取りに行きます

## トラブルシュート

- `unauthorized` / `failed to create subscription`
  - Twitch の OAuth スコープが足りない可能性
  - このアプリは `channel:read:redemptions` を要求します
- `redirect_uri does not match`
  - Twitch 開発者コンソールに登録した Redirect URL と config.toml が完全一致しているか確認

