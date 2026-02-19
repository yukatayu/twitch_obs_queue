async function api(url) {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(await res.text());
  }
  return await res.json();
}

async function copyToClipboard(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (_) {
    // fallback
    const ta = document.createElement('textarea');
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(ta);
    return ok;
  }
}

function render(rewards) {
  const root = document.getElementById('list');
  root.innerHTML = '';

  if (!rewards.length) {
    const empty = document.createElement('div');
    empty.className = 'small';
    empty.textContent = '報酬が取得できませんでした（権限やチャンネル設定を確認）';
    root.appendChild(empty);
    return;
  }

  for (const r of rewards) {
    const card = document.createElement('div');
    card.className = 'item';

    const info = document.createElement('div');
    const name = document.createElement('div');
    name.className = 'name';
    name.textContent = r.title;

    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = `cost=${r.cost} / enabled=${r.is_enabled} / id=${r.id}`;

    info.appendChild(name);
    info.appendChild(meta);

    const spacer = document.createElement('div');
    spacer.className = 'spacer';

    const copy = document.createElement('button');
    copy.className = 'btn';
    copy.textContent = 'IDをコピー';
    copy.onclick = async () => {
      const ok = await copyToClipboard(r.id);
      copy.textContent = ok ? 'コピーしました' : 'コピー失敗';
      setTimeout(() => copy.textContent = 'IDをコピー', 900);
    };

    card.appendChild(info);
    card.appendChild(spacer);
    card.appendChild(copy);

    root.appendChild(card);
  }
}

async function load() {
  const hint = document.getElementById('hint');
  hint.textContent = 'loading...';

  const st = await api('/api/status');
  if (!st.authenticated) {
    hint.textContent = '未ログインです。先に /admin で Twitchログインしてください。';
    render([]);
    return;
  }

  const rewards = await api('/api/rewards');
  hint.textContent = 'config.toml の twitch.target_reward_ids に、使いたい報酬のIDを配列で設定して再起動してください。';
  render(rewards);
}

document.getElementById('reload').onclick = () => load();

load().catch(e => {
  document.getElementById('hint').textContent = `エラー: ${e.message}`;
});
