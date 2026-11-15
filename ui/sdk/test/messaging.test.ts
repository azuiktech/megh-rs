// Specification: messaging in the browser, signed in as a client talking to one member of the tenant.
//
// - The chat follows `tenants/{tenant}/clients/{client}/members/{member}` on the events stream, with the last 50
//   messages first; the browser's EventSource reconnects and catches up with `Last-Event-ID` by itself.
// - `on(listener)` hears the member's messages, `on_event(listener)` its events (typing and the like); the client's
//   own are not echoed back.
// - `send(body)` is `POST /tenants/{tenant}/members/{member}/messages` with `{body}`; `send_event(signal)` is
//   `POST …/events` with `{signal}`.
// - A send that cannot reach the server is queued, and the queue is sent in order when the browser is back online.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Chat } from '../src/messaging'

const base = 'http://api.test/api/v1'
const events = 'http://api.test/events'
const topic = 'tenants/t1/clients/c1/members/m1'

class FakeSource {
  static last: FakeSource
  listeners: Record<string, ((event: MessageEvent) => void)[]> = {}
  closed = false
  constructor(public url: string, public init?: EventSourceInit) {
    FakeSource.last = this
  }
  addEventListener(type: string, listener: (event: MessageEvent) => void) {
    ;(this.listeners[type] ??= []).push(listener)
  }
  emit(type: string, data: unknown) {
    ;(this.listeners[type] ?? []).forEach((listener) => listener({ data: JSON.stringify(data) } as MessageEvent))
  }
  close() {
    this.closed = true
  }
}

let requests: { path: string; method: string; body: unknown }[]
let online: boolean
let windowListeners: Record<string, (() => void)[]>

beforeEach(() => {
  requests = []
  online = true
  windowListeners = {}
  vi.stubGlobal('EventSource', FakeSource)
  vi.stubGlobal('window', { addEventListener: (type: string, listener: () => void) => (windowListeners[type] ??= []).push(listener) })
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    if (!online) throw new TypeError('Failed to fetch')
    requests.push({ path: url.replace(base, ''), method: init?.method ?? 'GET', body: init?.body ? JSON.parse(init.body as string) : undefined })
    return new Response(null, { status: 204 })
  }))
})

afterEach(() => vi.unstubAllGlobals())

const open = () => new Chat({ base, events, tenant: 't1', client: 'c1', member: 'm1' })
const fromMember = { kind: 'message', id: 'x1', from: { member: 'm1' }, to: { client: 'c1' }, body: [{ version: '1.0' }], created_at: '2026-11-15T10:00:00Z' }
const fromMe = { ...fromMember, id: 'x2', from: { client: 'c1' }, to: { member: 'm1' } }

describe('chat', () => {
  it('follows its thread with the latest history, signed in with cookies', () => {
    open()
    expect(FakeSource.last.url).toBe(`${events}/stream?topic=${encodeURIComponent(topic)}&history_limit=50`)
    expect(FakeSource.last.init).toEqual({ withCredentials: true })
  })

  it('hears the member messages and events, not its own', () => {
    const chat = open()
    const heard: unknown[] = []
    const noticed: unknown[] = []
    chat.on((message) => heard.push(message))
    chat.on_event((notice) => noticed.push(notice))
    FakeSource.last.emit(topic, fromMember)
    FakeSource.last.emit(topic, fromMe)
    FakeSource.last.emit(topic, { kind: 'event', from: { member: 'm1' }, to: { client: 'c1' }, signal: 'typing_on' })
    FakeSource.last.emit(topic, { kind: 'event', from: { client: 'c1' }, to: { member: 'm1' }, signal: 'typing_on' })
    expect(heard).toEqual([fromMember])
    expect(noticed).toEqual([{ kind: 'event', from: { member: 'm1' }, to: { client: 'c1' }, signal: 'typing_on' }])
  })

  it('sends messages and events to its member', async () => {
    const chat = open()
    await chat.send({ name: 'approve', context: {} })
    await chat.send_event('typing_on')
    expect(requests).toEqual([
      { path: '/tenants/t1/members/m1/messages', method: 'POST', body: { body: { name: 'approve', context: {} } } },
      { path: '/tenants/t1/members/m1/events', method: 'POST', body: { signal: 'typing_on' } },
    ])
  })

  it('queues what it cannot send and sends it in order when back online', async () => {
    const chat = open()
    online = false
    await chat.send({ n: 1 })
    await chat.send({ n: 2 })
    expect(requests).toEqual([])
    online = true
    windowListeners.online.forEach((listener) => listener())
    await vi.waitFor(() => expect(requests.map((r) => r.body)).toEqual([{ body: { n: 1 } }, { body: { n: 2 } }]))
  })

  it('stops following when closed', () => {
    open().close()
    expect(FakeSource.last.closed).toBe(true)
  })
})
