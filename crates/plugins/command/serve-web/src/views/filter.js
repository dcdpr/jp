const field = document.getElementById('filter');
const entries = [...document.querySelectorAll('.conversation-list li')];
const noMatches = document.getElementById('no-matches');

const apply = () => {
  const needle = field.value.trim().toLowerCase();
  let shown = 0;

  for (const entry of entries) {
    const title = entry.querySelector('.title').textContent.toLowerCase();
    const match = title.includes(needle);
    entry.hidden = !match;
    if (match) shown++;
  }

  noMatches.hidden = shown > 0;
};

field.addEventListener('input', apply);

// Browsers restore a field's value on a back navigation without firing `input`,
// which would otherwise leave the text sitting above an unfiltered list.
apply();
