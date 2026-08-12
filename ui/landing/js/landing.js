function copy() {
  const el = document.querySelector('code');
  navigator.clipboard.writeText(el.textContent).then(() => {
    const btn = document.querySelector('.copy-btn');
    btn.textContent = 'Copied!';
    setTimeout(() => btn.textContent = 'Copy', 2000);
  });
}
