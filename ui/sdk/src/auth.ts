export interface ConnectedAccount {
  account_id: string
  provider:   string
  email:      string
}

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
      const res = await this.#fetch(this.#paths.me)
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
    const res = await this.#fetch(this.#paths.login, {
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
    await this.#fetch(this.#paths.logout, { method: 'POST' })
    this.#setUser(null)
  }

  /**
   * Connect an OAuth provider to the current user's account.
   * Pass extra `scopes` to request additional permissions (e.g. calendar, drive).
   * Opens a popup (or redirect) to the provider's consent page.
   * On success the popup closes and the ConnectedAccount is returned.
   */
  async connectWithOAuth(provider: string, scopes: string[] = [], mode: SignInMode = 'auto'): Promise<ConnectedAccount> {
    const params = scopes.length ? '?' + scopes.map(s => 'scope=' + encodeURIComponent(s)).join('&') : ''
    const connectUrl = `${this.#base}/auth/${provider}/connect${params}`

    if (mode === 'redirect') {
      this.#redirectToOAuth(connectUrl)
      return Promise.reject(new Error('Redirecting'))
    }

    const popup = window.open(connectUrl, `megh-${provider}-connect`, 'width=520,height=620,left=200,top=100')
    const blocked = !popup || popup.closed
    if (blocked) {
      if (mode === 'popup') throw new Error('Popup blocked — allow popups for this site, or use redirect mode')
      this.#redirectToOAuth(connectUrl)
      return Promise.reject(new Error('Redirecting'))
    }

    return new Promise((resolve, reject) => {
      const backendOrigin = new URL(this.#base).origin
      const onMessage = (event: MessageEvent) => {
        if (event.origin !== backendOrigin) return
        if (event.data?.type !== 'oauth_connect_success') return
        cleanup()
        event.data.account ? resolve(event.data.account as ConnectedAccount) : reject(new Error('Connect completed without account payload'))
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
    const res = await this.#fetch(`/auth/${provider}/disconnect`, {
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
   * Revoke an OAuth token for a provider and sign out.
   */
  async revokeOAuth(provider: string, token?: string): Promise<void> {
    const res = await this.#fetch(`/auth/${provider}/revoke`, {
      method: 'POST',
      body: token ? JSON.stringify({ token }) : undefined,
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
  async revokeGoogle(token?: string): Promise<void> {
    return this.revokeOAuth('google', token)
  }

  #redirectToOAuth(loginUrl: string): void {
    // Encode the current page URL so the backend (megh) can redirect back here
    // after OAuth completes. Falls back to '/' for backends that ignore return_to.
    const returnTo = encodeURIComponent(location.href)
    window.location.href = `${loginUrl}?return_to=${returnTo}`
  }

  #setUser(user: User | null): void {
    this.#user = user
    for (const fn of this.#listeners) fn(user)
  }

  #fetch(path: string, init?: RequestInit): Promise<Response> {
    return fetch(`${this.#base}${path}`, {
      ...init,
      credentials: 'include',
      headers: { 'Content-Type': 'application/json', ...init?.headers },
    })
  }
}
