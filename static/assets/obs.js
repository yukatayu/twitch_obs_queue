async function fetchQueue() {
  const res = await fetch('/api/queue');
  if (!res.ok) {
    throw new Error(await res.text());
  }
  return await res.json();
}

function render(items) {
  const root = document.getElementById('root');
  root.innerHTML = '';

  for (const item of items) {
    const el = document.createElement('div');
    el.className = 'item';

    const img = document.createElement('img');
    img.src = item.profile_image_url;
    img.loading = 'lazy';

    const name = document.createElement('div');
    name.className = 'name';
    name.textContent = item.display_name;

    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.textContent = `最近の参加: ${item.recent_participation_count}`;

    el.appendChild(img);
    el.appendChild(name);
    el.appendChild(meta);
    root.appendChild(el);
  }
}

async function loop() {
  try {
    const items = await fetchQueue();
    render(items);
  } catch (e) {
    // OBS overlay: silently ignore and retry
  }
  setTimeout(loop, 1000);
}

loop();
