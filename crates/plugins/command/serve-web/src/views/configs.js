// The ordered list of `--cfg` arguments a message runs under.
//
// Handlers are delegated from the document rather than bound to a row: the
// composer's chooser is fetched after this script runs, and every row added
// afterwards carries controls of its own.

// A fresh row of the given kind, ready to append.
function cloneRow(list, kind) {
  const template = list.querySelector(`.config-template[data-kind="${kind}"]`);
  return template ? template.content.firstElementChild.cloneNode(true) : null;
}

// Whether an argument names one of the configurations the picker offers.
//
// The picker carries an empty option for a row nothing has been chosen in, so
// the empty string has to be excluded by hand.
function isNamed(list, argument) {
  if (argument === '') return false;

  const template = list.querySelector('.config-template[data-kind="named"]');
  return !!template?.content.querySelector(`option[value="${CSS.escape(argument)}"]`);
}

// Append a row of the given kind holding `argument`, and hand it back.
function appendRow(list, kind, argument) {
  const items = list.querySelector('.config-items');
  const row = cloneRow(list, kind);
  if (!items || !row) return null;

  if (argument) row.querySelector('[name="cfg"]').value = argument;
  items.append(row);

  return row;
}

// `+ Value` writes its own row; `+ Configuration` opens the picker.
//
// A workspace carries dozens of configurations and a list usually wants several
// of them, so they are ticked as a set rather than chosen one dropdown at a
// time.
document.addEventListener('click', (event) => {
  const target = /** @type {Element} */ (event.target);
  const add = /** @type {HTMLButtonElement | null} */ (target.closest('.config-add'));
  if (!add) return;

  const list = add.closest('.config-list');
  if (!list) return;

  if (add.dataset.kind === 'named') {
    openPicker(list);
    return;
  }

  appendRow(list, 'value', '')?.querySelector('[name="cfg"]').focus();
});

// What has been ticked in a picker, in the order it was ticked.
//
// Kept beside the boxes rather than read off them: a checkbox knows whether it
// is ticked, not when, and the order the rows arrive in is the whole point.
const pickerOrder = new WeakMap();

// Open the picker with nothing ticked and nothing filtered out, whatever the
// last visit left behind.
function openPicker(list) {
  const picker = list.querySelector('.config-picker');
  if (!picker) return;

  pickerOrder.set(picker, []);
  for (const box of picker.querySelectorAll('input[type="checkbox"]')) box.checked = false;

  const filter = picker.querySelector('.config-filter');
  if (filter) filter.value = '';

  markPickerOrder(picker);
  filterPicker(picker, '');
  picker.showModal();
}

// Show the entries whose segment contains `query`, and hide the rest.
//
// Matched against the whole segment rather than the name alone, so a directory
// narrows to its own entries: `personas` leaves the personas, `dev` leaves every
// `dev` under any of them.
function filterPicker(picker, query) {
  const needle = query.trim().toLowerCase();

  for (const option of picker.querySelectorAll('.config-option')) {
    const box = option.querySelector('input[type="checkbox"]');
    option.hidden = needle !== '' && !box.dataset.segment.toLowerCase().includes(needle);
  }

  // A directory with nothing left under it has nothing left to head.
  for (const group of picker.querySelectorAll('fieldset')) {
    group.hidden = !group.querySelector('.config-option:not([hidden])');
  }
}

// The box a bare Enter would tick: the first one the filter has left.
function firstMatch(picker) {
  return picker.querySelector('.config-option:not([hidden]) input[type="checkbox"]');
}

document.addEventListener('input', (event) => {
  const target = /** @type {Element} */ (event.target);
  const filter = /** @type {HTMLInputElement | null} */ (target.closest?.('.config-filter'));
  if (!filter) return;

  filterPicker(filter.closest('.config-picker'), filter.value);
});

// Enter ticks the first match and clears the filter, so a set is gathered by
// typing a word per configuration rather than by aiming at boxes.
document.addEventListener('keydown', (event) => {
  const target = /** @type {Element} */ (event.target);
  const filter = /** @type {HTMLInputElement | null} */ (target.closest?.('.config-filter'));
  if (!filter || event.key !== 'Enter') return;

  // Held down, Enter means the set is finished rather than one more of it.
  if (event.metaKey || event.ctrlKey) return;

  // Otherwise this submits the form the picker sits inside.
  event.preventDefault();

  const picker = filter.closest('.config-picker');
  const box = firstMatch(picker);

  // A synthetic event rather than the bookkeeping inline: assigning `checked`
  // fires nothing, and the handler below owns what the order is.
  if (box && !box.checked) {
    box.checked = true;
    box.dispatchEvent(new Event('change', { bubbles: true }));
  }

  filter.value = '';
  filterPicker(picker, '');
});

// Cmd+Enter adds the ticked set, from anywhere in the picker.
//
// The picker is opened, filled and finished without the pointer ever arriving:
// the Add button is what the same thing looks like to a mouse.
document.addEventListener('keydown', (event) => {
  if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return;

  const picker = /** @type {Element} */ (event.target).closest?.('.config-picker');
  if (!picker) return;

  event.preventDefault();
  applyPicker(picker);
});

// Number each ticked box with the place its row will take, and clear the rest.
function markPickerOrder(picker) {
  const order = pickerOrder.get(picker) ?? [];

  for (const option of picker.querySelectorAll('.config-option')) {
    const box = option.querySelector('input[type="checkbox"]');
    const at = order.indexOf(box.dataset.segment);

    option.querySelector('.config-order').textContent = at < 0 ? '' : String(at + 1);
  }
}

document.addEventListener('change', (event) => {
  const target = /** @type {Element} */ (event.target);
  const box = /** @type {HTMLInputElement | null} */ (
    target.closest?.('.config-picker input[type="checkbox"]')
  );
  if (!box) return;

  const picker = box.closest('.config-picker');
  const order = pickerOrder.get(picker) ?? [];
  const at = order.indexOf(box.dataset.segment);

  if (box.checked && at < 0) {
    order.push(box.dataset.segment);
  } else if (!box.checked && at >= 0) {
    order.splice(at, 1);
  }

  pickerOrder.set(picker, order);
  markPickerOrder(picker);
});

// Add a row per ticked box, in the order they were ticked, and close.
function applyPicker(picker) {
  const list = picker.closest('.config-list');
  if (!list) return;

  for (const segment of pickerOrder.get(picker) ?? []) appendRow(list, 'named', segment);

  picker.close();
}

document.addEventListener('click', (event) => {
  const target = /** @type {Element} */ (event.target);
  const button = target.closest('.config-picker-add, .config-picker-cancel');
  if (!button) return;

  const picker = /** @type {HTMLDialogElement | null} */ (button.closest('.config-picker'));
  if (!picker) return;

  if (button.classList.contains('config-picker-cancel')) {
    picker.close();
    return;
  }

  applyPicker(picker);
});

document.addEventListener('click', (event) => {
  const remove = /** @type {Element} */ (event.target).closest('.config-remove');
  if (remove) remove.closest('.config-item')?.remove();
});

// Reordering is what the list is for: the host applies the arguments in the
// order they arrive, so which row sits above which decides what wins.
//
// The row is dragged by its grip, and moved a place at a time by the arrow keys
// while the grip has focus. A drag is the faster of the two and the keyboard is
// the only one that works without a pointer, so both are here.

// The state of the drag in progress, or null between drags.
//
// One at a time: a second finger on a second grip is ignored until the first
// lets go.
let configDrag = null;

// Where an element sits halfway down, in viewport coordinates.
function middleOf(element) {
  const box = element.getBoundingClientRect();
  return box.top + box.height / 2;
}

// Put `row` on the other side of the neighbour it now covers more than half of,
// and keep going while that stays true: one move of a pointer can cross several
// rows, and a fast drag would otherwise leave the row behind.
//
// Moving a row changes where the list lays it out, so the origin the drag
// measures from is shifted by as much. Without that the row would jump by its
// own height at every swap instead of staying under the pointer.
function settle(drag, clientY) {
  // Bounded rather than a bare loop: every swap is meant to reduce the distance
  // left to cover, but rows of unequal height could argue about who is halfway
  // over whom, and a page that hangs mid-drag is worse than one that stops
  // reordering.
  for (let step = 0; step < drag.items.children.length; step += 1) {
    const box = drag.row.getBoundingClientRect();
    const above = drag.row.previousElementSibling;
    const below = drag.row.nextElementSibling;

    let neighbour = null;
    if (above && box.top < middleOf(above)) {
      neighbour = above;
    } else if (below && box.bottom > middleOf(below)) {
      neighbour = below;
    }

    if (!neighbour) return;

    if (neighbour === above) {
      neighbour.before(drag.row);
    } else {
      neighbour.after(drag.row);
    }

    drag.origin += drag.row.getBoundingClientRect().top - box.top;
    drag.row.style.transform = `translateY(${clientY - drag.origin}px)`;
  }
}

// Pointer events rather than HTML5 drag-and-drop, which has no touch support at
// all. The grip declares `touch-action: none`, so a drag that starts on it
// drags rather than scrolling the page.
document.addEventListener('pointerdown', (event) => {
  const grip = /** @type {Element} */ (event.target).closest?.('.config-grip');
  if (!grip || configDrag || event.button !== 0) return;

  const row = grip.closest('.config-item');
  const items = row?.parentElement;
  if (!row || !items) return;

  // Keeps the press from selecting the text around it, and from starting the
  // browser's own drag of the glyph.
  event.preventDefault();
  grip.setPointerCapture(event.pointerId);

  configDrag = { grip, row, items, pointer: event.pointerId, origin: event.clientY };
  row.classList.add('is-dragging');
});

document.addEventListener('pointermove', (event) => {
  if (!configDrag || event.pointerId !== configDrag.pointer) return;

  configDrag.row.style.transform = `translateY(${event.clientY - configDrag.origin}px)`;
  settle(configDrag, event.clientY);
});

function endConfigDrag(event) {
  if (!configDrag || event.pointerId !== configDrag.pointer) return;

  configDrag.row.style.transform = '';
  configDrag.row.classList.remove('is-dragging');

  if (configDrag.grip.hasPointerCapture(event.pointerId)) {
    configDrag.grip.releasePointerCapture(event.pointerId);
  }

  configDrag = null;
}

document.addEventListener('pointerup', endConfigDrag);
document.addEventListener('pointercancel', endConfigDrag);

// The arrow keys move the row the focused grip belongs to.
document.addEventListener('keydown', (event) => {
  const target = /** @type {Element} */ (event.target);
  const grip = /** @type {HTMLButtonElement | null} */ (target.closest?.('.config-grip'));
  if (!grip) return;

  const up = event.key === 'ArrowUp';
  if (!up && event.key !== 'ArrowDown') return;

  const row = grip.closest('.config-item');
  const neighbour = up ? row?.previousElementSibling : row?.nextElementSibling;
  if (!neighbour) return;

  // Otherwise the page scrolls under the row that just moved.
  event.preventDefault();

  if (up) {
    neighbour.before(row);
  } else {
    neighbour.after(row);
  }

  // Moving a node drops the focus that was on it, and a row moved two places is
  // two presses of a key that would otherwise land somewhere else.
  grip.focus();
});

// Enter inside a row or the picker does nothing.
//
// Left alone it submits the form the chooser sits in, which is the composer's
// dialog — whose first submit button is Cancel, so the work would be thrown
// away — or the new-conversation form, which would start the conversation from
// a half-written message.
//
// A button is left alone: Enter is how one is pressed, which is how the picker
// is confirmed and how a row is dropped without a pointer.
document.addEventListener('keydown', (event) => {
  if (event.key !== 'Enter') return;

  const target = /** @type {Element} */ (event.target);
  if (!target.closest?.('.config-item, .config-picker')) return;
  if (target.closest('button')) return;

  event.preventDefault();
});

// Read and rewrite the list, for the composer's dialog: it holds the applied
// choice itself and puts it back when a later edit is cancelled.
window.jpConfigList = {
  // The arguments the list holds, in order.
  //
  // A row left on the picker's empty option contributes nothing, the same way
  // the server drops it.
  read(root) {
    // A named row's field is a `<select>`, a written one's an `<input>`.
    const fields = /** @type {NodeListOf<HTMLInputElement | HTMLSelectElement>} */ (
      root.querySelectorAll('.config-item [name="cfg"]')
    );

    return Array.from(fields)
      .map((field) => field.value.trim())
      .filter((argument) => argument !== '');
  },

  // Replace the rows with one per argument, in order.
  write(root, args) {
    const list = root.querySelector('.config-list');
    const items = list?.querySelector('.config-items');
    if (!items) return;

    items.replaceChildren();

    for (const argument of args) {
      appendRow(list, isNamed(list, argument) ? 'named' : 'value', argument);
    }
  },
};
