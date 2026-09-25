// Properties one page script puts on `window` for another to use.
//
// TypeScript does not let a script add a property to `window` by assigning it,
// so each one is declared here. Only `tsc` reads this file; nothing serves it.

interface Window {
  /** Read and rewrite a configuration list. Defined by `configs.js`. */
  jpConfigList: {
    /** The `--cfg` arguments the list under `root` holds, in order. */
    read(root: Element): string[];

    /** Replace the rows of the list under `root` with one per argument. */
    write(root: Element, args: string[]): void;
  };
}
