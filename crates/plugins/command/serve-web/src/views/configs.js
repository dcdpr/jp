// A `+` adds another key/value row below the one it belongs to.
//
// Delegated from the document rather than bound to each button: the dialog's
// chooser is fetched after this script runs, and every row it adds carries a
// button of its own.
document.addEventListener('click', (event) => {
  const add = event.target.closest('.config-add');
  if (!add) return;

  const row = add.closest('.config-pair');
  if (!row) return;

  const next = row.cloneNode(true);
  next.querySelectorAll('input').forEach((field) => { field.value = ''; });

  row.after(next);
  next.querySelector('input').focus();
});

// Enter inside an assignment row does nothing.
//
// Left alone it submits the form the row sits in, which is the dialog — whose
// first submit button is Cancel, so the row would be thrown away — or the
// new-conversation form, which would start the conversation from a half-written
// message.
document.addEventListener('keydown', (event) => {
  if (event.key !== 'Enter') return;
  if (!event.target.closest?.('.config-pair')) return;

  event.preventDefault();
});
