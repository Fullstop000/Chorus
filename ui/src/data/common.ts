export function buildQueryParams(params: Record<string, string | number | boolean | undefined>): string {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== '') {
      search.set(key, String(value))
    }
  }
  return search.toString()
}

export function queryString(params: Record<string, string | number | boolean | undefined>): string {
  const s = buildQueryParams(params)
  return s ? `?${s}` : ''
}

export function composeTransform<A, B>(fn: (a: A) => B): (input: A) => B
export function composeTransform<A, B, C>(
  first: (a: A) => B,
  second: (b: B) => C
): (input: A) => C
export function composeTransform<A, B, C, D>(
  first: (a: A) => B,
  second: (b: B) => C,
  third: (c: C) => D
): (input: A) => D
export function composeTransform(...fns: Array<(input: unknown) => unknown>) {
  return (input: unknown) => fns.reduce((acc, fn) => fn(acc), input)
}

export function identity<T>(value: T): T {
  return value
}
