const header = document.querySelector('.page-header');

// What the server says the list amounts to.
//
// A fingerprint rather than a count: renaming a conversation, or using one,
// leaves the count exactly where it was, and so would an archive and a creation
// between the same two visits.
async function listDigest() {
  const r = await fetch('/conversations/digest');
  if (!r.ok) throw new Error(r.status);

  const { digest } = await r.json();
  return digest;
}

async function reloadIfStale() {
  try {
    if (await listDigest() !== header.dataset.digest) location.reload();
  } catch (e) {
    // Offline, or the server is restarting. The next return tries again.
  }
}

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') reloadIfStale();
});

// The header is a way back to the top on platforms that do not do it themselves.
// Ignores the links inside it, which have somewhere else to go.
header.addEventListener('click', (event) => {
  if (!event.target.closest('a')) scrollTo(0, 0);
});

// Archiving asks first: a swipe is easy to make by accident, and a conversation
// is not something to lose to a stray gesture.
//
// Handled here rather than on the form so the confirmation is one dialog for the
// whole list rather than one per row.
document.addEventListener('submit', async (event) => {
  const form = event.target.closest('.row-actions');
  if (!form) return;

  event.preventDefault();

  const row = form.closest('li');
  const title = row.querySelector('.title').textContent.trim();
  if (!confirm('Archive ' + title + '?')) return;

  try {
    const r = await fetch(form.action, {
      method: 'POST',
      headers: { accept: 'application/json' },
    });
    if (!r.ok) throw new Error(r.status);

    // Removed rather than reloaded: the rest of the list is unchanged, and a
    // reload would lose the filter and the scroll position.
    row.remove();

    // The page now matches a list it has not seen, so its fingerprint has to
    // catch up — otherwise the next return here reloads to show the removal it
    // is already showing. Asked for rather than computed, since the page holds
    // no list to compute one from.
    try {
      header.dataset.digest = await listDigest();
    } catch (e) {
      // Left stale, which costs one reload on the next return and nothing else.
    }
  } catch (e) {
    alert('Could not archive that conversation.');
  }
});

// A tap on a row that is swiped open should close it rather than follow the
// link, which is what every list with this gesture does.
document.addEventListener('click', (event) => {
  const entry = event.target.closest('.row-entry');
  if (!entry) return;

  const track = entry.closest('.row-track');
  if (track.scrollLeft > 4) {
    event.preventDefault();
    track.scrollTo({ left: 0, behavior: 'smooth' });
  }
});
