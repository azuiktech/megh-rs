import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Auth } from '../src/auth'

const base = 'http://api.test'
let requests: { path: string; init: RequestInit | undefined }[]

beforeEach(() => {
  requests = []
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    requests.push({ path: url.replace(base, ''), init })
    return new Response(JSON.stringify({ id: 'u1' }), { status: 200 })
  }))
})

afterEach(() => vi.unstubAllGlobals())

describe('writes', () => {
  it('carry the session cookie and no CSRF token, and never fetch one', async () => {
    const auth = new Auth({ base })
    await auth.fetch('/things', { method: 'POST' })
    expect(requests.map((r) => r.path)).toEqual(['/things'])
    const { init } = requests[0]
    expect(init?.credentials).toBe('include')
    expect(init?.headers as Record<string, string>).not.toHaveProperty('x-csrf-token')
  })

  it('need no extra request around signing out and signing in', async () => {
    const auth = new Auth({ base })
    await auth.signOut()
    await auth.signInWithPassword('a@example.com', 'secret')
    expect(requests.map((r) => r.path)).toEqual(['/auth/logout', '/auth/login', '/auth/me'])
  })
})
