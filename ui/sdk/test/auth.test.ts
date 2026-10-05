import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Auth } from '../src/auth'

const base = 'http://api.test'
let tokens: string[]
let writes: { path: string; token: string | undefined }[]

function respond(status: number, body: unknown = {}): Response {
  return new Response(JSON.stringify(body), { status })
}

beforeEach(() => {
  tokens = ['token-1', 'token-2', 'token-3']
  writes = []
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    const path = url.replace(base, '')
    if (path === '/auth/csrf-token') return new Response(tokens.shift() ?? 'none')
    if (path === '/auth/me') return respond(200, { id: 'u1' })
    const headers = (init?.headers ?? {}) as Record<string, string>
    writes.push({ path, token: headers['x-csrf-token'] })
    return respond(200)
  }))
})

afterEach(() => vi.unstubAllGlobals())

describe('the CSRF token after sign-out', () => {
  it('is fetched again for the next write, because signing out ends the session the token belonged to', async () => {
    const auth = new Auth({ base })
    await auth.signOut()
    await auth.signInWithPassword('a@example.com', 'secret')
    expect(writes).toEqual([
      { path: '/auth/logout', token: 'token-1' },
      { path: '/auth/login', token: 'token-2' },
    ])
  })

  it('is reused for several writes in one session', async () => {
    const auth = new Auth({ base })
    await auth.fetch('/things', { method: 'POST' })
    await auth.fetch('/things', { method: 'POST' })
    expect(writes.map((w) => w.token)).toEqual(['token-1', 'token-1'])
  })

  it('is fetched again after a write is refused with 403', async () => {
    const refusals = [403]
    vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
      const path = url.replace(base, '')
      if (path === '/auth/csrf-token') return new Response(tokens.shift() ?? 'none')
      const headers = (init?.headers ?? {}) as Record<string, string>
      writes.push({ path, token: headers['x-csrf-token'] })
      return respond(refusals.shift() ?? 200)
    }))
    const auth = new Auth({ base })
    await auth.fetch('/things', { method: 'POST' })
    await auth.fetch('/things', { method: 'POST' })
    expect(writes.map((w) => w.token)).toEqual(['token-1', 'token-2'])
  })
})
