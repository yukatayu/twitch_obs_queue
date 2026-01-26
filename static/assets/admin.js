async function api(method, url, body) {
  const opts = { method, headers: {} };
  if (body !== undefined) {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(url, opts);
  if (!res.ok) {
    throw new Error(await res.text());
  }
  if (res.status === 204) return null;
  return await res.json();
}

function setText(id, text) {
  document.getElementById(id).textContent = text;
}

function renderQueue(items) {
  const root = document.getElementById('queue');
  root.innerHTML = '';

  if (!items.length) {
    const empty = document.createElement('div');
    empty.className = 'small';
    empty.textContent = '空です';
    root.appendChild(empty);
    return;
  }

  items.sort((a,b) => a.position - b.position);

  for (const item of items) {
    const row = document.createElement('div');
    row.className = 'item';

    const img = document.createElement('img');
    img.src = item.profile_image_url;
    img.loading = 'lazy';

    const info = document.createElement('div');
    const name = document.createElement('div');
    name.className = 'name';
    name.textContent = item.display_name;

    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = `@${item.user_login} / 最近の参加: ${item.recent_participation_count}`;

    info.appendChild(name);
    info.appendChild(meta);

    const spacer = document.createElement('div');
    spacer.className = 'spacer';

    const up = document.createElement('button');
    up.className = 'btn';
    up.textContent = '↑';
    up.onclick = async () => {
      await api('POST', `/api/queue/${item.id}/move_up`);
      await refresh();
    };

    const down = document.createElement('button');
    down.className = 'btn';
    down.textContent = '↓';
    down.onclick = async () => {
      await api('POST', `/api/queue/${item.id}/move_down`);
      await refresh();
    };

    const complete = document.createElement('button');
    complete.className = 'btn';
    complete.textContent = '✅完了';
    complete.onclick = async () => {
      await api('POST', `/api/queue/${item.id}/delete`, { mode: 'completed' });
      await refresh();
    };

    const cancel = document.createElement('button');
    cancel.className = 'btn danger';
    cancel.textContent = '❌キャンセル';
    cancel.onclick = async () => {
      await api('POST', `/api/queue/${item.id}/delete`, { mode: 'canceled' });
      await refresh();
    };

    row.appendChild(img);
    row.appendChild(info);
    row.appendChild(spacer);
    row.appendChild(up);
    row.appendChild(down);
    row.appendChild(complete);
    row.appendChild(cancel);

    root.appendChild(row);
  }
}

let lastStatus = null;

async function refresh() {
  try {
    lastStatus = await api('GET', '/api/status');
    const auth = lastStatus.authenticated ? 'ログイン済み' : '未ログイン';
    const b = lastStatus.broadcaster_login ? ` / broadcaster: ${lastStatus.broadcaster_login}` : '';
    const w = ` / window: ${lastStatus.participation_window_secs}s`;
    const reward = lastStatus.target_reward_id && lastStatus.target_reward_id.trim() !== ''
      ? ` / target_reward_id: ${lastStatus.target_reward_id}`
      : ' / target_reward_id: (未設定)';

    setText('statusText', `${auth}${b}${w}${reward}`);

    const hint = document.getElementById('hint');
    if (!lastStatus.authenticated) {
      hint.textContent = 'まず「Twitchでログイン」を押してください。';
    } else if (!lastStatus.target_reward_id || lastStatus.target_reward_id.trim() === '') {
      hint.textContent = 'config.toml の twitch.target_reward_id が未設定です。右上の「報酬ID一覧」で確認して設定してください。';
    } else {
      hint.textContent = '';
    }

    const items = await api('GET', '/api/queue');
    renderQueue(items);

    document.getElementById('loginBtn').style.display = lastStatus.authenticated ? 'none' : '';
    document.getElementById('logoutBtn').style.display = lastStatus.authenticated ? '' : 'none';
  } catch (e) {
    setText('statusText', `エラー: ${e.message}`);
  }
}

// Buttons

document.getElementById('loginBtn').onclick = () => {
  location.href = '/auth/start';
};

document.getElementById('logoutBtn').onclick = async () => {
  try {
    await api('POST', '/auth/logout');
  } catch (e) {}
  await refresh();
};

async function loop() {
  await refresh();
  setTimeout(loop, 1500);
}

loop();
