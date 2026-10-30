// Specification: a tenant's connections from the browser (issue #234).
//
// - `connections(tenant)` lists them: `GET /tenants/{tenant}/connections`.
// - `connect(tenant, { url, bearer } | { url, api_key })` adds a site: `POST /tenants/{tenant}/connections`.
// - `connect(tenant, { url, oauth: true })` signs in at the site in a popup opened on
//   `/tenants/{tenant}/connections/oauth?url=`, and resolves with the connection the popup posts.
// - `reconnect(tenant, id, credentials)` is `PUT /tenants/{tenant}/connections/{id}`; with `{ oauth: true, url }` it
//   signs in again in a popup on `…/connections/oauth?url=&id=`; `disconnect(tenant, id)` is
//   `POST …/{id}:disconnect`, `remove(tenant, id)` is `DELETE …/{id}`.
// - `connectWithOAuth(provider, scopes, mode, tenant)` connects a known provider's account for the tenant.
// - A refused request throws the server's reason.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Auth } from '../src/auth'

const base = 'http://api.test/api/v1'
const tenant = 't1'
let requests: { path: string; method: string; body: unknown }[]
let answer: { status: number; body: unknown }

beforeEach(() => {
  requests = []
  answer = { status: 200, body: [] }
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    requests.push({ path: url.replace(base, ''), method: init?.method ?? 'GET', body: init?.body ? JSON.parse(init.body as string) : undefined })
    return new Response(answer.status === 204 ? null : JSON.stringify(answer.body), { status: answer.status })
  }))
})

afterEach(() => vi.unstubAllGlobals())

/** A browser whose popups are recorded and which can post a message as the backend. */
function browser() {
  const opened: string[] = []
  const listeners: ((event: MessageEvent) => void)[] = []
  vi.stubGlobal('window', {
    open: (url: string) => (opened.push(url), { closed: false }),
    addEventListener: (_: string, listener: (event: MessageEvent) => void) => listeners.push(listener),
    removeEventListener: () => {},
  })
  const post = (data: unknown) => listeners.forEach((listener) => listener({ origin: 'http://api.test', data } as MessageEvent))
  return { opened, post }
}

describe('connections', () => {
  it('are listed for the tenant', async () => {
    answer.body = [{ id: 'c1', provider: 'url', connected: true }]
    const listed = await new Auth({ base }).connections(tenant)
    expect(requests).toEqual([{ path: '/tenants/t1/connections', method: 'GET', body: undefined }])
    expect(listed).toEqual(answer.body)
  })

  it('to a site are added with a bearer token or an API key', async () => {
    const auth = new Auth({ base })
    answer.body = { id: 'c1' }
    await auth.connect(tenant, { url: 'https://crm.example', bearer: { token: 'tok' } })
    await auth.connect(tenant, { url: 'https://shop.example', api_key: { key: 'k', header: 'X-Shop-Key' } })
    expect(requests).toEqual([
      { path: '/tenants/t1/connections', method: 'POST', body: { url: 'https://crm.example', bearer: { token: 'tok' } } },
      { path: '/tenants/t1/connections', method: 'POST', body: { url: 'https://shop.example', api_key: { key: 'k', header: 'X-Shop-Key' } } },
    ])
  })

  it('to a site by OAuth sign in in a popup and resolve with the connection', async () => {
    const { opened, post } = browser()
    const connecting = new Auth({ base }).connect(tenant, { url: 'https://mcp.example/mcp', oauth: true }, 'popup')
    post({ type: 'oauth_connect_success', connection: { id: 'c9', auth: 'oauth' } })
    expect(await connecting).toEqual({ id: 'c9', auth: 'oauth' })
    expect(opened).toEqual(['http://api.test/api/v1/tenants/t1/connections/oauth?url=https%3A%2F%2Fmcp.example%2Fmcp'])
  })

  it('are reconnected, disconnected and removed', async () => {
    const auth = new Auth({ base })
    answer.body = { id: 'c1' }
    await auth.reconnect(tenant, 'c1', { bearer: { token: 'new' } })
    await auth.disconnect(tenant, 'c1')
    answer.status = 204
    answer.body = null
    await auth.remove(tenant, 'c1')
    expect(requests).toEqual([
      { path: '/tenants/t1/connections/c1', method: 'PUT', body: { bearer: { token: 'new' } } },
      { path: '/tenants/t1/connections/c1:disconnect', method: 'POST', body: undefined },
      { path: '/tenants/t1/connections/c1', method: 'DELETE', body: undefined },
    ])
  })

  it('by OAuth are connected again by signing in again', async () => {
    const { opened, post } = browser()
    const connecting = new Auth({ base }).reconnect(tenant, 'c9', { oauth: true, url: 'https://mcp.example/mcp' }, 'popup')
    post({ type: 'oauth_connect_success', connection: { id: 'c9', connected: true } })
    expect(await connecting).toEqual({ id: 'c9', connected: true })
    expect(opened).toEqual(['http://api.test/api/v1/tenants/t1/connections/oauth?url=https%3A%2F%2Fmcp.example%2Fmcp&id=c9'])
  })

  it('of a known provider are made for the tenant', async () => {
    const { opened, post } = browser()
    const connecting = new Auth({ base }).connectWithOAuth('google', ['gmail'], 'popup', tenant)
    post({ type: 'oauth_connect_success', account: { account_id: 'a1', provider: 'google', email: 'orders@shop.example' } })
    await connecting
    expect(opened).toEqual(['http://api.test/api/v1/auth/google/connect?scope=gmail&tenant_id=t1'])
  })

  it('throw the reason a request is refused', async () => {
    answer = { status: 400, body: { error: 'url must be an http(s) URL' } }
    await expect(new Auth({ base }).connect(tenant, { url: 'nope', bearer: { token: 't' } })).rejects.toThrow('url must be an http(s) URL')
  })
})
