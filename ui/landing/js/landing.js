  // ── Carbon calculator ─────────────────────────────────────────────
  const hoursEl = document.getElementById('hours');
  const tokensEl = document.getElementById('tokens');
  const savingsEl = document.getElementById('savings');
  const co2El = document.getElementById('co2-result');

  function update() {
    const hours = +hoursEl.value;
    const tokens = +tokensEl.value;
    const savings = +savingsEl.value / 100;

    document.getElementById('hours-out').textContent = hours + 'h';
    document.getElementById('tokens-out').textContent = tokens.toLocaleString();
    document.getElementById('savings-out').textContent = (savings * 100) + '%';

    // Rough estimate: interactions/day = hours * 10 (6 min each)
    // tokens_saved_per_day = interactions * tokens_per_interaction * savings_rate
    // annual_co2_kg = tokens_saved_per_day * 365 * 0.4 / 1000 / 1000
    const interactionsPerDay = hours * 10;
    const tokensSavedPerDay = interactionsPerDay * tokens * savings;
    const annualKg = tokensSavedPerDay * 365 * 0.4 / 1_000_000;

    co2El.textContent = annualKg < 1
      ? (annualKg * 1000).toFixed(0) + ' g'
      : annualKg >= 1000
        ? (annualKg / 1000).toFixed(1) + ' metric tons'
        : annualKg.toFixed(1) + ' kg';
  }

  hoursEl.addEventListener('input', update);
  tokensEl.addEventListener('input', update);
  savingsEl.addEventListener('input', update);
  update();
