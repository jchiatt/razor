# Devices and paired transport

`crate::devices` sits below the controller, children and API. One `Devices`, opened by the
controller before sessions restart, owns validated enrollment, durable replacement and revocation.

```rust
/// Enrollment and paired connections shared by every server operation.
pub(crate) struct Devices;
impl Devices {
    /// Missing enrollment is empty; unreadable or malformed enrollment refuses startup.
    pub async fn open(home: &Path) -> Result<Self, Error>;
    /// Current direct pairings with receivers closed when their enrollment is removed.
    pub async fn paired(&self) -> Vec<(Device, watch::Receiver<()>)>;
    /// One current direct pairing, without cloning unrelated devices or subscribing streams.
    pub async fn peer(&self, name: &str) -> Option<Device>;
    /// A matching key's device and revocation receiver; the local key has no device name.
    pub async fn authorize(&self, key: &[u8]) -> Option<(watch::Receiver<()>, Option<String>)>;
    /// Enrollment without private keys or notification tokens, safe for a client to list.
    pub async fn list(&self) -> Vec<Device>;
    /// Current notification preferences, defaulting after a device is removed.
    pub async fn notifications(&self, name: &str) -> Notifications;
    /// Changes an existing device only after persistence succeeds; removed devices stay removed.
    pub async fn change(&self, name: &str, change: impl FnOnce(&mut Device)) -> Result<(), Error>;
    /// Persist before publishing enrollment changes or ending revoked streams.
    pub async fn enroll(&self, name: String, address: Option<String>, key: Option<String>) -> Result<Device, Error>;
    pub async fn revoke(&self, name: &str) -> Result<(), Error>;
    /// Authenticate to a direct peer through the shared pool, without redirects or retries.
    pub fn request(&self, device: &Device, method: Method, path: &str) -> RequestBuilder;
    /// Bounded JSON within two seconds; a fresh status line, independent of the API cache.
    pub async fn fetch<T: DeserializeOwned>(&self, device: &Device, path: &str, query: &impl Serialize, bound: usize) -> Option<T>;
    pub async fn snapshot(&self, device: &Device) -> Option<Vec<AgentView>>;
    /// Single attempts with operation-specific deadlines and acknowledgment wording.
    pub async fn child(&self, server: &str, operation: &ChildRequest) -> Result<Value, Error>;
    pub async fn mail(&self, server: &str, mail: &Mail) -> Result<String, Error>;
}
```

1. The controller opens one `Arc<Devices>` and shares it with its API and agent operations.
2. Only this module reads `devices.json`; a missing file means an empty enrollment.
3. API enrollment and revocation delegate to the same locked, synced replacement as phone changes.
4. A successful mutation publishes memory only after its durable rename succeeds.
5. Authorization and peer subscriptions receive the existing per-device revocation receiver.
6. Mail, children, search, recall and status lookup read current enrollment from memory.
7. Tool lookup requests a fresh status line; the API keeps its bounded cached peer snapshots.
8. Requests share bearer authentication and disable redirects; mutations are never retried.
9. API forwarding retains its one-hop guard, header deadline and streaming cancellation.
10. Post rings its local controller directly; it has no separate peer HTTP path.

The hard decision is three clients built once under this owner: reads and relay reuse their
pool with two seconds to connect; child mutations use a separate client with the same connect
deadline and no idle connections; mail has five seconds to connect and no idle connections.
A fresh connection lets a refused mutation say it was not sent, even after an earlier read's
connection went silent. Children keep thirty seconds overall, mail ten, and bounded reads two.
One universal client would change a deadline or give up read reuse. There is no second
enrollment cache or per-call client builder.

The existing notification-token, follow and open-stream operations move with enrollment; their
revocation channels and limits are retained. `list` redacts credentials and notification tokens;
`notifications` supplies the defaults after removal, so neither is a second enrollment reader.
The API reuses this module's error type; transport failures retain their cause and context, and
enrollment refusals retain the HTTP status the device routes return.
