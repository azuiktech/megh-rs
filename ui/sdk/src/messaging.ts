/** One side of a conversation: a member of the tenant, or a client. */
export type Party = { member: string } | { client: string }

/** What is said without a message: typing, or that messages up to one were read. */
export type Signal = 'typing_on' | 'typing_off' | { read: string }

export interface Message {
  kind: 'message'
  id: string
  from: Party
  to: Party
  /** a2ui 1.0 messages, or the action a renderer sent back. */
  body: unknown
  created_at: string
}

export interface Notice {
  kind: 'event'
  from: Party
  to: Party
  signal: Signal
  /** a2ui updates that turn the read mark, on a read. */
  updates?: unknown[]
}

export interface ChatConfig {
  /** The API prefix, such as `https://api.example.com/api/v1`. */
  base: string
  /** Where the events router is mounted, such as `https://api.example.com/events`. */
  events: string
  tenant: string
  /** The signed-in client. */
  client: string
  /** The member this client talks to. */
  member: string
}

type Outgoing = { path: string; payload: unknown }

/** A client's conversation with one member: hears it over the events stream, sends over HTTP, and keeps what it
 *  cannot send until the browser is back online. */
export class Chat {
  #config: ChatConfig
  #source: EventSource
  #queue: Outgoing[] = []
  #flushing: Promise<void> | undefined
  #messages: ((message: Message) => void)[] = []
  #notices: ((notice: Notice) => void)[] = []

  constructor(config: ChatConfig) {
    this.#config = config
    const topic = `tenants/${config.tenant}/clients/${config.client}/members/${config.member}`
    this.#source = new EventSource(`${config.events}/stream?topic=${encodeURIComponent(topic)}&history_limit=50`, { withCredentials: true })
    this.#source.addEventListener(topic, (event) => this.#deliver(JSON.parse((event as MessageEvent).data)))
    window.addEventListener('online', () => void this.#flush())
  }

  on(listener: (message: Message) => void): void {
    this.#messages.push(listener)
  }

  on_event(listener: (notice: Notice) => void): void {
    this.#notices.push(listener)
  }

  send(body: unknown): Promise<void> {
    return this.#enqueue({ path: 'messages', payload: { body } })
  }

  send_event(signal: Signal): Promise<void> {
    return this.#enqueue({ path: 'events', payload: { signal } })
  }

  close(): void {
    this.#source.close()
  }

  #deliver(delivery: Message | Notice): void {
    const mine = 'client' in delivery.from && delivery.from.client === this.#config.client
    if (mine) return
    if (delivery.kind === 'message') this.#messages.forEach((listener) => listener(delivery))
    else this.#notices.forEach((listener) => listener(delivery))
  }

  #enqueue(outgoing: Outgoing): Promise<void> {
    this.#queue.push(outgoing)
    return this.#flush()
  }

  /** Sends the queue in order; stops at the first send that cannot reach the server and keeps the rest. */
  #flush(): Promise<void> {
    this.#flushing ??= this.#drain().finally(() => (this.#flushing = undefined))
    return this.#flushing
  }

  async #drain(): Promise<void> {
    const [first] = this.#queue
    if (!first || !(await this.#post(first))) return
    this.#queue.shift()
    return this.#drain()
  }

  async #post({ path, payload }: Outgoing): Promise<boolean> {
    const { base, tenant, member } = this.#config
    const url = `${base}/tenants/${tenant}/members/${member}/${path}`
    const init: RequestInit = { method: 'POST', credentials: 'include', headers: { 'content-type': 'application/json' }, body: JSON.stringify(payload) }
    return fetch(url, init).then(() => true, () => false)
  }
}
