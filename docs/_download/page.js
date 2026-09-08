(() => {
  const scene = document.querySelector('.transfer-scene');
  const replay = document.querySelector('#replay');
  const state = document.querySelector('#transfer-state');
  const percent = document.querySelector('#transfer-percent');
  const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
  if (!scene || !replay || !state || !percent) return;
  const duration = 4600;
  let frame = 0;
  const finish = () => {
    cancelAnimationFrame(frame);
    scene.style.setProperty('--progress', '100%');
    scene.classList.remove('is-running');
    state.textContent = state.dataset.complete;
    percent.textContent = '100%';
    replay.disabled = false;
    replay.textContent = `${replay.dataset.label} ↗`;
  };
  const play = () => {
    if (reducedMotion.matches) { finish(); return; }
    replay.disabled = true;
    replay.textContent = replay.dataset.running;
    scene.classList.add('is-running');
    state.textContent = state.dataset.sending;
    const start = performance.now();
    const animate = (now) => {
      const elapsed = Math.min((now - start) / duration, 1);
      const value = Math.floor((1 - Math.pow(1 - elapsed, 1.5)) * 100);
      scene.style.setProperty('--progress', `${value}%`);
      percent.textContent = `${value}%`;
      if (elapsed < 1) frame = requestAnimationFrame(animate);
      else finish();
    };
    frame = requestAnimationFrame(animate);
  };
  replay.addEventListener('click', play);
  reducedMotion.addEventListener('change', finish);
  document.addEventListener('visibilitychange', () => { if (document.hidden) finish(); });
  if (!reducedMotion.matches) play();
})();
