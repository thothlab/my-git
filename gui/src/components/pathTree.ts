/**
 * Path layout: the one place that splits a `/`-separated path into folders and
 * collapses a chain of single-child directories into one row.
 *
 * Both file trees in the app go through here — the Changes panel
 * (`ChangesView.tsx`, `FileStatus`) and the commit's file list
 * (`log/CommitDetailsPane.tsx`, `CommitFileEntry`) — so the module is generic
 * over the item and knows nothing but `path`.
 *
 * The branch tree (`log/BranchTree.tsx`) deliberately stays outside; its
 * docblock says why.
 *
 * One rule worth naming, because the two former copies disagreed on it:
 * **directory paths are the paths of the rows that exist**, i.e. of the
 * compacted nodes — see `treeDirPaths`. A chain `a/b/c` merged into one row has
 * one key (`a/b/c`), not three. Collapse-all names rows; the intermediate
 * segments `a` and `a/b` are never drawn and never carry collapse state.
 */
export interface FileTreeNode<T> {
  name: string;
  path: string;
  dirs: FileTreeNode<T>[];
  files: T[];
}

export const baseName = (p: string) => p.split("/").pop() || p;

/** Build a directory tree from flat file paths. */
export function buildFileTree<T extends { path: string }>(files: T[]): FileTreeNode<T> {
  const root: FileTreeNode<T> = { name: "", path: "", dirs: [], files: [] };
  for (const f of files) {
    const parts = f.path.split("/");
    parts.pop(); // drop the file name
    let node = root;
    let acc = "";
    for (const seg of parts) {
      acc = acc ? `${acc}/${seg}` : seg;
      let child = node.dirs.find((d) => d.name === seg);
      if (!child) {
        child = { name: seg, path: acc, dirs: [], files: [] };
        node.dirs.push(child);
      }
      node = child;
    }
    node.files.push(f);
  }
  // The root itself never merges: its children are the top-level modules.
  root.dirs = root.dirs.map(compactDir);
  return root;
}

/** Merge single-child directory chains (a/b/c) into one node, like Android Studio. */
export function compactDir<T>(node: FileTreeNode<T>): FileTreeNode<T> {
  let merged: FileTreeNode<T> = { ...node, dirs: node.dirs.map(compactDir) };
  while (merged.files.length === 0 && merged.dirs.length === 1) {
    const child = merged.dirs[0];
    merged = {
      name: `${merged.name}/${child.name}`,
      path: child.path,
      dirs: child.dirs,
      files: child.files,
    };
  }
  return merged;
}

/** Number of files under a node, including every subdirectory. */
export function countFiles<T>(node: FileTreeNode<T>): number {
  return node.files.length + node.dirs.reduce((n, d) => n + countFiles(d), 0);
}

/**
 * Paths of the nodes that actually get a row — the compacted ones, not every
 * intermediate segment. Collapse-all has to name the rows it collapses, and a
 * chain merged into one row has a single key: the deepest segment's path.
 */
export function treeDirPaths<T>(node: FileTreeNode<T>): string[] {
  const out: string[] = [];
  const walk = (n: FileTreeNode<T>) => {
    for (const d of n.dirs) {
      out.push(d.path);
      walk(d);
    }
  };
  walk(node);
  return out;
}
