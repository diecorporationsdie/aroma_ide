export interface SemVer {
  readonly major: number;
  readonly minor: number;
  readonly patch: number;
}

const VERSION_RE = /^(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:-[0-9A-Za-z.-]+)?$/;
const RANGE_PREFIX_RE = /^(\^|~|>=|<=|>|<)\s*/;

export function parseVersion(input: string): SemVer | null {
  const m = VERSION_RE.exec(input.trim());
  if (!m) return null;
  return {
    major: Number(m[1]),
    minor: m[2] ? Number(m[2]) : 0,
    patch: m[3] ? Number(m[3]) : 0,
  };
}

function cmp(a: SemVer, b: SemVer): number {
  if (a.major !== b.major) return a.major - b.major;
  if (a.minor !== b.minor) return a.minor - b.minor;
  return a.patch - b.patch;
}

export function satisfies(actual: string, range: string): boolean {
  const a = parseVersion(actual);
  if (!a) return false;

  const r = range.trim();
  if (r === '' || r === '*' || r === 'x') return true;

  const prefix = RANGE_PREFIX_RE.exec(r);
  if (prefix) {
    const target = parseVersion(r.slice(prefix[0].length));
    if (!target) return false;
    const c = cmp(a, target);
    switch (prefix[1]) {
      case '>=':
        return c >= 0;
      case '<=':
        return c <= 0;
      case '>':
        return c > 0;
      case '<':
        return c < 0;
      case '^':
        return a.major === target.major && c >= 0;
      case '~':
        return a.major === target.major && a.minor === target.minor && c >= 0;
    }
  }

  const exact = parseVersion(r);
  if (!exact) return false;
  return cmp(a, exact) === 0;
}
