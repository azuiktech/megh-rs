export interface EventsConfig {
  /** Backend base URL, e.g. 'http://localhost:8080'. No trailing slash. */
  base: string
  /**
   * Endpoint path where events are mounted.
   * Defaults to '/events'.
   */
  path?: string
}

export interface EventMessage<T = unknown> {
  id?: string
  topic: string
  data: T
  timestamp?: string
}

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected'

export type EventListener<T = unknown> = (event: EventMessage<T>) => void
export type StatusListener = (status: ConnectionStatus) => void

export interface HistoryConfig {
  /** RFC3339 timestamp string or Date object */
  since?: string | Date
  /** Maximum number of historical records to fetch on initial subscribe */
  limit?: number
}

export interface SubscribeOptions<T = unknown> {
  history?: HistoryConfig
  onEvent: EventListener<T>
}

/**
 * Match evaluates whether topic satisfies pattern.
 * Supports:
 *   - '*' for matching a single path segment (e.g. 'invoices/*')
 *   - '**' for matching zero or more path segments (e.g. 'invoices/**')
 */
export function matchTopic(pattern: string, topic: string): boolean {
  if (pattern === topic || pattern === '*' || pattern === '**') {
    return true
  }
  if (!pattern || !topic) {
    return pattern === topic
  }

  const patParts = pattern.split('/')
  const topParts = topic.split('/')

  function matchSegments(pat: string[], top: string[]): boolean {
    if (pat.length === 0) return top.length === 0
    if (pat[0] === '**') {
      if (pat.length === 1) return true
      for (let i = 0; i <= top.length; i++) {
        if (matchSegments(pat.slice(1), top.slice(i))) {
          return true
        }
      }
      return false
    }
    if (top.length === 0) return false
    if (pat[0] === '*' || pat[0] === top[0]) {
      return matchSegments(pat.slice(1), top.slice(1))
    }
    return false
  }

  return matchSegments(patParts, topParts)
}

export class Events {
  readonly #base: string
  readonly #path: string
  #eventSource: EventSource | null = null
  #clientId: string | null = null
  #lastEventId: string | null = null
  #status: ConnectionStatus = 'disconnected'
  #subscriptions = new Map<string, Set<EventListener<any>>>()
  #statusListeners: StatusListener[] = []
  #connectingPromise: Promise<string> | null = null

  constructor(config: EventsConfig) {
    this.#base = config.base.replace(/\/$/, '')
    this.#path = config.path ? (config.path.startsWith('/') ? config.path : `/${config.path}`) : '/events'
  }

  get status(): ConnectionStatus {
    return this.#status
  }

  get clientId(): string | null {
    return this.#clientId
  }

  get lastEventId(): string | null {
    return this.#lastEventId
  }

  /**
   * Subscribe to topic events matching a wildcard pattern.
   * Accepts either a direct listener function (live only) or SubscribeOptions (past history + live).
   * Returns a cleanup function that unsubscribes when called.
   */
  subscribe<T = unknown>(
    pattern: string,
    listenerOrOptions: EventListener<T> | SubscribeOptions<T>,
  ): () => void {
    const listener =
      typeof listenerOrOptions === 'function'
        ? listenerOrOptions
        : listenerOrOptions.onEvent

    const historyConfig =
      typeof listenerOrOptions === 'object' && listenerOrOptions.history
        ? listenerOrOptions.history
        : undefined

    let listeners = this.#subscriptions.get(pattern)
    const isNewPattern = !listeners || listeners.size === 0

    if (!listeners) {
      listeners = new Set()
      this.#subscriptions.set(pattern, listeners)
    }
    listeners.add(listener as EventListener<any>)

    this.#ensureConnected(historyConfig ? [pattern] : [], historyConfig)
      .then((clientId) => {
        if (isNewPattern && clientId) {
          this.#syncSubscriptions(clientId, [pattern], [])
        }
      })
      .catch(() => {})

    return () => {
      this.unsubscribe(pattern, listener as EventListener<any>)
    }
  }

  /**
   * Unsubscribe a listener or an entire pattern.
   */
  unsubscribe(pattern: string, listener?: EventListener<any>): void {
    const listeners = this.#subscriptions.get(pattern)
    if (!listeners) return

    if (listener) {
      listeners.delete(listener)
    } else {
      listeners.clear()
    }

    if (listeners.size === 0) {
      this.#subscriptions.delete(pattern)
      if (this.#clientId) {
        this.#syncSubscriptions(this.#clientId, [], [pattern])
      }
    }
  }

  /**
   * Listen to connection status changes.
   */
  onStatusChange(fn: StatusListener): () => void {
    this.#statusListeners.push(fn)
    fn(this.#status)
    return () => {
      this.#statusListeners = this.#statusListeners.filter((l) => l !== fn)
    }
  }

  /**
   * Explicitly disconnect the SSE stream.
   */
  disconnect(): void {
    if (this.#eventSource) {
      this.#eventSource.close()
      this.#eventSource = null
    }
    this.#clientId = null
    this.#connectingPromise = null
    this.#setStatus('disconnected')
  }

  #ensureConnected(initialTopics: string[] = [], history?: HistoryConfig): Promise<string> {
    if (this.#clientId && this.#eventSource && this.#eventSource.readyState === EventSource.OPEN) {
      return Promise.resolve(this.#clientId)
    }
    if (this.#connectingPromise) {
      return this.#connectingPromise
    }

    this.#setStatus('connecting')

    const params = new URLSearchParams()
    if (this.#lastEventId) {
      params.set('since', this.#lastEventId)
    }
    for (const t of initialTopics) {
      params.append('topic', t)
    }
    if (history) {
      if (history.since) {
        const sinceStr = history.since instanceof Date ? history.since.toISOString() : history.since
        params.set('history_since', sinceStr)
      }
      if (history.limit) {
        params.set('history_limit', String(history.limit))
      }
    }

    const query = params.toString()
    const url = `${this.#base}${this.#path}/stream${query ? `?${query}` : ''}`

    this.#connectingPromise = new Promise((resolve, reject) => {
      const es = new EventSource(url, { withCredentials: true })
      this.#eventSource = es

      es.addEventListener('connect', (e: MessageEvent) => {
        try {
          const payload = JSON.parse(e.data) as { clientId: string }
          this.#clientId = payload.clientId
          this.#setStatus('connected')

          // Resubscribe all active patterns on initial connection or reconnect
          const activePatterns = Array.from(this.#subscriptions.keys())
          if (activePatterns.length > 0 && this.#clientId) {
            this.#syncSubscriptions(this.#clientId, activePatterns, [])
          }

          resolve(this.#clientId)
        } catch (err) {
          reject(err)
        }
      })

      es.addEventListener('message', (e: MessageEvent) => {
        try {
          if (e.lastEventId) {
            this.#lastEventId = e.lastEventId
          }
          const event = JSON.parse(e.data) as EventMessage<any>
          if (event.id) {
            this.#lastEventId = event.id
          }
          this.#dispatchEvent(event)
        } catch {
          // ignore unparseable events
        }
      })

      es.onerror = () => {
        this.#setStatus('connecting')
      }
    })

    return this.#connectingPromise
  }

  #dispatchEvent(event: EventMessage<any>): void {
    for (const [pattern, listeners] of this.#subscriptions.entries()) {
      if (matchTopic(pattern, event.topic)) {
        for (const listener of listeners) {
          try {
            listener(event)
          } catch (err) {
            console.error('Error in event listener:', err)
          }
        }
      }
    }
  }

  async #syncSubscriptions(clientId: string, subscribe: string[], unsubscribe: string[]): Promise<void> {
    try {
      await fetch(`${this.#base}${this.#path}/sub`, {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ clientId, subscribe, unsubscribe, lastEventId: this.#lastEventId }),
      })
    } catch (err) {
      console.error('Failed to sync subscriptions:', err)
    }
  }

  #setStatus(status: ConnectionStatus): void {
    if (this.#status === status) return
    this.#status = status
    for (const fn of this.#statusListeners) {
      fn(status)
    }
  }
}
