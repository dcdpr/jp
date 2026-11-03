// Why `jp vite serve` can't trust this site to read its documents, or `null`.
//
// Set when the plugins that write the documents were built from other site
// files than these, or when these aren't the ones `jp vite` writes. The RFD and
// ticket pages show it in place of their content, and the dev server refuses
// writes. Always `null` in a static build.
export const problem = __JP_VITE_PROBLEM__
