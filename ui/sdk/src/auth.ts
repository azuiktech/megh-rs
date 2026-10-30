export interface ConnectedAccount {
  account_id: string
  provider:   string
  email:      string
}

/** A tenant's account at a known provider or a site, as its card shows it: never its token. */
export interface Connection {
  id:         string
  provider:   string
  account_id: string
  email:      string | null
  url:        string | null
  spec_url:   string | null
  icon_url:   string | null
  auth:       'oauth' | 'bearer' | 'api_key' | null
  key_header: string | null
  connected:  boolean
  created_at: string | null
}

/** How a site signs its requests: a bearer token, or an API key sent in `header` (default `X-API-Key`). */
export type SiteCredentials = { bearer: { token: string } } | { api_key: { key: string; header?: string } }

/** A site to connect, with its OpenAPI document: by its credentials, or by signing in there with OAuth. */
export type Site = { url: string; spec_url?: string } & (SiteCredentials | { oauth: true })

export interface AuthConfig<User> {
  /** Backend base URL, e.g. 'http://localhost:8080'. No trailing slash. */
  base: string
  /**
   * Override route paths when the backend mounts them somewhere other than /auth/*.
   * Example for agentivity: { me: '/api/v1/me', login: '/api/v1/session', ... }
   */
  paths?: {
    me?: string           // default: /auth/me
    login?: string        // default: /auth/login
    logout?: string       // default: /auth/logout
    googleLogin?: string  // default: /auth/google/login
    googleRevoke?: string // default: /auth/google/revoke
  }
  /**
   * Map the raw /me response to the User type.
   * Use when the server wraps the user: e.g. (data) => data.user
   * Defaults to the response body as-is.
   */
  transform?: (raw: unknown) => User
}

export type SignInMode = 'popup' | 'redirect' | 'auto'

type Listener<User> = (user: User | null) => void

export class Auth<User = Record<string, unknown>> {
  readonly #base: string
  readonly #paths: Required<NonNullable<AuthConfig<User>['paths']>>
  readonly #transform: (raw: unknown) => User
  #user: User | null = null
  #listeners: Listener<User>[] = []

  constructor(config: AuthConfig<User>) {
    this.#base = config.base.replace(/\/$/, '')
    this.#paths = {
      me:           config.paths?.me           ?? '/auth/me',
      login:        config.paths?.login        ?? '/auth/login',
      logout:       config.paths?.logout       ?? '/auth/logout',
      googleLogin:  config.paths?.googleLogin  ?? '/auth/google/login',
      googleRevoke: config.paths?.googleRevoke ?? '/auth/google/revoke',
    }
    this.#transform = config.transform ?? ((raw) => raw as User)
  }

  get user(): User | null {
    return this.#user
  }

  /**
   * Subscribe to auth-state changes. The callback fires immediately with the
   * current user, then on every subsequent change. Returns an unsubscribe fn.
   */
  onChange(fn: Listener<User>): () => void {
    this.#listeners.push(fn)
    fn(this.#user)
    return () => {
      this.#listeners = this.#listeners.filter((l) => l !== fn)
    }
  }

  /**
   * Restore session from the server's session cookie. Call once on page load.
   * Silently resolves to null if no session exists.
   */
  async restore(): Promise<void> {
    try {
      const res = await this.fetch(this.#paths.me)
      this.#setUser(res.ok ? this.#transform(await res.json()) : null)
    } catch {
      this.#setUser(null)
    }
  }

  /**
   * Sign in with any OAuth2 provider mounted at /auth/<provider>/login.
   *
   * mode 'auto' (default): tries a popup; falls back to full-page redirect if
   *   the browser blocks the popup (common on mobile and some desktop configs).
   * mode 'popup': always use a popup. Rejects if blocked.
   * mode 'redirect': always use full-page redirect.
   */
  async signInWithOAuth(provider: string, mode: SignInMode = 'auto'): Promise<void> {
    const loginUrl = `${this.#base}/auth/${provider}/login`

    if (mode === 'redirect') {
      this.#redirectToOAuth(loginUrl)
      return
    }

    const popup = window.open(loginUrl, `megh-${provider}-auth`, 'width=520,height=620,left=200,top=100')

    // Detect popup blocked: open() returns null, or the window is immediately closed.
    const blocked = !popup || popup.closed
    if (blocked) {
      if (mode === 'popup') {
        throw new Error('Popup blocked — allow popups for this site, or use redirect mode')
      }
      // mode === 'auto': fall back to redirect
      this.#redirectToOAuth(loginUrl)
      return
    }

    return new Promise((resolve, reject) => {
      const backendOrigin = new URL(this.#base).origin

      const onMessage = (event: MessageEvent) => {
        if (event.origin !== backendOrigin) return
        if (event.data?.type !== 'oauth_success') return
        cleanup()
        if (event.data.user) {
          this.#setUser(event.data.user as User)
          resolve()
        } else {
          reject(new Error('OAuth completed without a user payload'))
        }
      }

      // Detect popup closed by user before completing.
      const poll = setInterval(() => {
        if (popup.closed) {
          cleanup()
          reject(new Error('Sign-in cancelled'))
        }
      }, 500)

      const cleanup = () => {
        clearInterval(poll)
        window.removeEventListener('message', onMessage)
      }

      window.addEventListener('message', onMessage)
    })
  }

  /**
   * Sign in with Google OAuth (shorthand for signInWithOAuth('google')).
   */
  async signInWithGoogle(mode: SignInMode = 'auto'): Promise<void> {
    return this.signInWithOAuth('google', mode)
  }

  /**
   * Sign in with email + password (HTTP Basic auth).
   * Only works when the server has basic auth enabled (non-production).
   */
  async signInWithPassword(email: string, password: string): Promise<void> {
    const res = await this.fetch(this.#paths.login, {
      method: 'POST',
      headers: { Authorization: `Basic ${btoa(`${email}:${password}`)}` },
    })
    if (!res.ok) {
      const body = await res.json().catch(() => ({})) as { error?: string }
      throw new Error(body.error ?? 'Sign-in failed')
    }
    // Restore from /me so the transform + consistent User shape applies
    // regardless of what the login endpoint's own response body looks like.
    await this.restore()
  }

  /** Sign out — clears the session cookie and notifies listeners. */
  async signOut(): Promise<void> {
    await this.fetch(this.#paths.logout, { method: 'POST' })
    this.#setUser(null)
  }

  /**
   * Connect an OAuth provider to the current user's account.
   * Pass extra `scopes` to request additional permissions (e.g. calendar, drive).
   * Opens a popup (or redirect) to the provider's consent page.
   * On success the popup closes and the ConnectedAccount is returned.
   */
  async connectWithOAuth(provider: string, scopes: string[] = [], mode: SignInMode = 'auto', tenant?: string): Promise<ConnectedAccount> {
    const params = new URLSearchParams(scopes.map((scope) => ['scope', scope]))
    if (tenant) params.append('tenant_id', tenant)
    const query = params.size ? `?${params}` : ''
    const data = await this.#connectInPopup(`${this.#base}/auth/${provider}/connect${query}`, `megh-${provider}-connect`, mode)
    if (!data.account) throw new Error('Connect completed without account payload')
    return data.account as ConnectedAccount
  }

  /** The tenant's connections, connected or not. */
  async connections(tenant: string): Promise<Connection[]> {
    return this.#json(await this.fetch(this.#connectionsPath(tenant)), 'Listing connections failed')
  }

  /**
   * Connect the tenant to a site: with a bearer token or an API key, or `oauth: true` to sign in there in a popup
   * (megh finds the site's OAuth server and registers with it).
   */
  async connect(tenant: string, site: Site, mode: SignInMode = 'auto'): Promise<Connection> {
    if ('oauth' in site) {
      const query = new URLSearchParams({ url: site.url, ...(site.spec_url ? { spec_url: site.spec_url } : {}) })
      const url = `${this.#base}${this.#connectionsPath(tenant)}/oauth?${query}`
      return this.#connection(await this.#connectInPopup(url, 'megh-site-connect', mode))
    }
    const res = await this.fetch(this.#connectionsPath(tenant), { method: 'POST', body: JSON.stringify(site) })
    return this.#json(res, 'Connect failed')
  }

  /** Connect the tenant's site connection `id` again: with new credentials, or by signing in at `url` again. */
  async reconnect(tenant: string, id: string, credentials: SiteCredentials | { oauth: true; url: string }, mode: SignInMode = 'auto'): Promise<Connection> {
    if ('oauth' in credentials) {
      const url = `${this.#base}${this.#connectionsPath(tenant)}/oauth?${new URLSearchParams({ url: credentials.url, id })}`
      return this.#connection(await this.#connectInPopup(url, 'megh-site-connect', mode))
    }
    const res = await this.fetch(`${this.#connectionsPath(tenant)}/${id}`, { method: 'PUT', body: JSON.stringify(credentials) })
    return this.#json(res, 'Reconnect failed')
  }

  #connection(data: Record<string, unknown>): Connection {
    if (!data.connection) throw new Error('Connect completed without connection payload')
    return data.connection as Connection
  }

  /** Disconnect the tenant's connection `id`; its card stays. */
  async disconnect(tenant: string, id: string): Promise<Connection> {
    return this.#json(await this.fetch(`${this.#connectionsPath(tenant)}/${id}:disconnect`, { method: 'POST' }), 'Disconnect failed')
  }

  /** Disconnect and delete the tenant's connection `id`, card and all. */
  async remove(tenant: string, id: string): Promise<void> {
    const res = await this.fetch(`${this.#connectionsPath(tenant)}/${id}`, { method: 'DELETE' })
    if (!res.ok) throw new Error(await this.#reason(res, 'Remove failed'))
  }

  #connectionsPath(tenant: string): string {
    return `/tenants/${encodeURIComponent(tenant)}/connections`
  }

  async #json<T>(res: Response, fallback: string): Promise<T> {
    if (!res.ok) throw new Error(await this.#reason(res, fallback))
    return res.json() as Promise<T>
  }

  async #reason(res: Response, fallback: string): Promise<string> {
    const body = await res.json().catch(() => ({})) as { error?: string }
    return body.error ?? fallback
  }

  /** Opens `url` in a popup (or redirects) and resolves with what the backend posts on `oauth_connect_success`. */
  #connectInPopup(url: string, name: string, mode: SignInMode): Promise<Record<string, unknown>> {
    if (mode === 'redirect') {
      this.#redirectToOAuth(url)
      return Promise.reject(new Error('Redirecting'))
    }
    const popup = window.open(url, name, 'width=520,height=620,left=200,top=100')
    const blocked = !popup || popup.closed
    if (blocked) {
      if (mode === 'popup') return Promise.reject(new Error('Popup blocked — allow popups for this site, or use redirect mode'))
      this.#redirectToOAuth(url)
      return Promise.reject(new Error('Redirecting'))
    }
    return new Promise((resolve, reject) => {
      const backendOrigin = new URL(this.#base).origin
      const onMessage = (event: MessageEvent) => {
        if (event.origin !== backendOrigin) return
        if (event.data?.type !== 'oauth_connect_success') return
        cleanup()
        resolve(event.data)
      }
      const poll = setInterval(() => { if (popup.closed) { cleanup(); reject(new Error('Connect cancelled')) } }, 500)
      const cleanup = () => { clearInterval(poll); window.removeEventListener('message', onMessage) }
      window.addEventListener('message', onMessage)
    })
  }

  /** Connect Google to the current user's account (shorthand for connectWithOAuth('google', scopes)). */
  async connectWithGoogle(scopes: string[] = [], mode: SignInMode = 'auto'): Promise<ConnectedAccount> {
    return this.connectWithOAuth('google', scopes, mode)
  }

  /** Disconnect a provider from the current user's account. */
  async disconnectWithOAuth(provider: string, accountId: string): Promise<void> {
    const res = await this.fetch(`/auth/${provider}/disconnect`, {
      method: 'POST',
      body: JSON.stringify({ account_id: accountId }),
    })
    if (!res.ok) {
      const body = await res.json().catch(() => ({})) as { error?: string }
      throw new Error(body.error ?? 'Disconnect failed')
    }
  }

  /** Disconnect Google from the current user's account (shorthand for disconnectWithOAuth('google')). */
  async disconnectWithGoogle(accountId: string): Promise<void> {
    return this.disconnectWithOAuth('google', accountId)
  }

  /**
   * Revoke the stored token of the current user's account at a provider and sign out.
   */
  async revokeOAuth(provider: string, accountId: string): Promise<void> {
    const res = await this.fetch(`/auth/${provider}/revoke`, {
      method: 'POST',
      body: JSON.stringify({ account_id: accountId }),
    })
    if (!res.ok) {
      const body = await res.json().catch(() => ({})) as { error?: string }
      throw new Error(body.error ?? 'Token revocation failed')
    }
    this.#setUser(null)
  }

  /**
   * Revoke Google OAuth token and sign out (shorthand for revokeOAuth('google')).
   */
  async revokeGoogle(accountId: string): Promise<void> {
    return this.revokeOAuth('google', accountId)
  }

  #redirectToOAuth(loginUrl: string): void {
    // Encode the current page URL so the backend (megh) can redirect back here
    // after OAuth completes. Falls back to '/' for backends that ignore return_to.
    const url = new URL(loginUrl)
    url.searchParams.set('return_to', location.href)
    window.location.href = url.toString()
  }

  #setUser(user: User | null): void {
    this.#user = user
    for (const fn of this.#listeners) fn(user)
  }

  /** `fetch` against the server's base URL with the session cookie. */
  async fetch(path: string, init?: RequestInit): Promise<Response> {
    const contentType = init?.body instanceof FormData ? {} : { 'Content-Type': 'application/json' }
    const headers = { ...contentType, ...init?.headers } as Record<string, string>
    return fetch(`${this.#base}${path}`, { ...init, credentials: 'include', headers })
  }
}
