// Cmd+Enter starts the conversation, as it sends a message on the conversation
// page.
//
// `requestSubmit` rather than `submit`: it runs the form's own validation, so a
// blank message is reported where it was typed instead of posted and refused.
document.addEventListener('keydown', (event) => {
  if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return;

  // A dialog takes the same chord for finishing its own work. Read off the
  // keystroke rather than off which dialogs are open, which by now may include
  // the one that just closed itself on this very key.
  if (event.target.closest?.('dialog')) return;

  const form = document.querySelector('.new-conversation');
  if (!form) return;

  event.preventDefault();
  form.requestSubmit();
});
