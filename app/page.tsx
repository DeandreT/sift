import Image from "next/image";
import {
  ArrowRight,
  BookOpen,
  Boxes,
  Code2,
  KeyRound,
  Layers3,
  Maximize2,
  Radio,
  RotateCcw,
  Search,
  ShieldCheck,
  Terminal,
  Waypoints,
} from "lucide-react";

const repository = "https://github.com/DeandreT/sift";
const assetBase = process.env.NEXT_PUBLIC_BASE_PATH ?? "";

const capabilities = [
  {
    icon: Waypoints,
    number: "01",
    title: "See the namespace",
    text: "Search queues, topics, subscriptions, and rules across multiple live connections. Runtime counts and status stay attached to the entity that owns them.",
  },
  {
    icon: Search,
    number: "02",
    title: "Read the payload",
    text: "Inspect JSON, XML, text, gzip, base64-wrapped, AMQP value, and binary bodies while keeping the original bytes available for faithful resend.",
  },
  {
    icon: Radio,
    number: "03",
    title: "Control the message",
    text: "Peek, receive, settle, defer, schedule, cancel, dead-letter, resend, or resubmit with the sequence numbers and lock state visible.",
  },
  {
    icon: RotateCcw,
    number: "04",
    title: "Recover deliberately",
    text: "Purge and dead-letter resubmission run in cancellable background operations with live progress instead of freezing the workspace.",
  },
  {
    icon: Boxes,
    number: "05",
    title: "Move definitions",
    text: "Export namespace entity descriptions to versioned JSON, then recreate missing entities or overwrite existing definitions in another namespace.",
  },
  {
    icon: KeyRound,
    number: "06",
    title: "Keep secrets separate",
    text: "Profiles live in TOML. Connection strings live in the operating system credential store and fall back to memory when no keyring is available.",
  },
];

const layers = [
  {
    label: "Native UI",
    name: "sift",
    detail: "egui views and operator state",
  },
  {
    label: "Async boundary",
    name: "backend",
    detail: "typed commands, events, and cancellation",
  },
  {
    label: "Protocols",
    name: "mgmt + AMQP",
    detail: "ATOM/XML over HTTPS and Service Bus messaging",
  },
  {
    label: "Service",
    name: "Azure Service Bus",
    detail: "namespaces, entities, sessions, and messages",
  },
];

export default function Home() {
  return (
    <>
      <a className="skip-link" href="#main">
        Skip to content
      </a>

      <header className="site-header">
        <div className="wrap nav-inner">
          <a className="brand" href="#top" aria-label="sift home">
            <span className="brand-mark" aria-hidden="true">
              <i />
              <i />
              <i />
            </span>
            <span>sift</span>
          </a>

          <nav className="desktop-nav" aria-label="Primary navigation">
            <a href="#workspace">Workspace</a>
            <a href="#capabilities">Capabilities</a>
            <a href="#architecture">Architecture</a>
            <a className="nav-source" href={repository}>
              <Code2 size={17} aria-hidden="true" />
              Source
            </a>
          </nav>

          <details className="mobile-nav">
            <summary aria-label="Open navigation">
              <span />
              <span />
              <span />
            </summary>
            <nav aria-label="Mobile navigation">
              <a href="#workspace">Workspace</a>
              <a href="#capabilities">Capabilities</a>
              <a href="#architecture">Architecture</a>
              <a href={repository}>Source</a>
            </nav>
          </details>
        </div>
      </header>

      <main id="main">
        <section className="hero" id="top">
          <Image
            className="hero-image"
            src={`${assetBase}/sift-connect.png`}
            alt=""
            fill
            priority
            loading="eager"
            sizes="100vw"
          />
          <div className="hero-shade" aria-hidden="true" />
          <div className="hero-grid" aria-hidden="true" />
          <div className="wrap hero-inner">
            <p className="eyebrow">
              <span />
              Native Azure Service Bus explorer
            </p>
            <h1>sift</h1>
            <p className="hero-statement">
              See what is moving.
              <br />
              Know what happens next.
            </p>
            <p className="hero-copy">
              Namespace structure, message bodies, dead-letter queues, sessions,
              and runtime state in one focused Rust desktop application.
            </p>
            <div className="hero-actions">
              <a className="button button-primary" href="#start">
                Build from source
                <ArrowRight size={18} aria-hidden="true" />
              </a>
              <a
                className="button button-secondary"
                href={`${repository}/blob/main/docs/architecture.md`}
              >
                <BookOpen size={18} aria-hidden="true" />
                Read architecture
              </a>
            </div>
            <div className="release-line">
              <span>v0.1.0</span>
              <p>Development preview</p>
            </div>
          </div>
        </section>

        <section className="signal-strip" aria-label="Project attributes">
          <div className="wrap signal-inner">
            <span>Rust + egui</span>
            <span>Multi-namespace</span>
            <span>AMQP / WebSockets</span>
            <span>MIT licensed</span>
          </div>
        </section>

        <section className="workspace-section" id="workspace">
          <div className="wrap">
            <div className="section-heading">
              <div>
                <p className="section-kicker">The working surface</p>
                <h2>A native console for the message in front of you.</h2>
              </div>
              <p>
                sift keeps high-frequency inspection and operational actions
                close together. Open entities in dockable tabs, detach a view
                into its own window, and keep the namespace tree available while
                work continues in the backend.
              </p>
            </div>

            <figure className="product-shot demo-frame">
              <div className="shot-bar">
                <span className="shot-status">
                  <i />
                  Live application
                </span>
                <a
                  className="demo-fullscreen"
                  href={`${assetBase}/app/`}
                  target="_blank"
                  rel="noreferrer"
                >
                  <Maximize2 size={14} aria-hidden="true" />
                  Open full screen
                </a>
              </div>
              <div className="demo-shell">
                <iframe
                  src={`${assetBase}/app/`}
                  title="Interactive sift application demo"
                  loading="lazy"
                  allow="clipboard-write"
                />
              </div>
              <figcaption className="demo-caption">
                <span>In-memory namespace</span>
                <span>Simulation emits a message every 3.5 seconds</span>
              </figcaption>
            </figure>

            <div className="workspace-notes">
              <article>
                <span>Tree</span>
                <h3>Structure stays visible</h3>
                <p>
                  Queues, topics, subscriptions, and rules load into one
                  searchable hierarchy scoped to each connection.
                </p>
              </article>
              <article>
                <span>Tabs</span>
                <h3>Context stays open</h3>
                <p>
                  Entity overview, live counts, messages, dead letters, and
                  sessions share a dockable workspace.
                </p>
              </article>
              <article>
                <span>Events</span>
                <h3>The UI stays responsive</h3>
                <p>
                  Network work runs on a dedicated Tokio thread and wakes egui
                  only when a typed result is ready.
                </p>
              </article>
            </div>
          </div>
        </section>

        <section className="capabilities-section" id="capabilities">
          <div className="wrap">
            <div className="section-heading compact">
              <div>
                <p className="section-kicker">Operator tools</p>
                <h2>Browse carefully. Act precisely.</h2>
              </div>
              <p>
                Read-only inspection and destructive actions are visually and
                behaviorally distinct. Locks, progress, confirmation, and raw
                payload fidelity are part of the workflow.
              </p>
            </div>

            <div className="capability-grid">
              {capabilities.map(({ icon: Icon, number, title, text }) => (
                <article key={number}>
                  <div className="capability-meta">
                    <span>{number}</span>
                    <Icon size={22} strokeWidth={1.7} aria-hidden="true" />
                  </div>
                  <h3>{title}</h3>
                  <p>{text}</p>
                </article>
              ))}
            </div>
          </div>
        </section>

        <section className="architecture-section" id="architecture">
          <div className="wrap architecture-inner">
            <div className="architecture-copy">
              <p className="section-kicker light">Runtime architecture</p>
              <h2>One clear boundary between pixels and protocols.</h2>
              <p>
                The UI submits typed commands. A dedicated Tokio backend owns
                HTTP and AMQP clients, reports typed events, and requests a
                repaint. Lower crates never depend on egui.
              </p>
              <a
                className="text-link"
                href={`${repository}/blob/main/docs/architecture.md`}
              >
                Explore the full design
                <ArrowRight size={18} aria-hidden="true" />
              </a>
            </div>

            <div className="layer-flow" aria-label="sift runtime layers">
              {layers.map((layer, index) => (
                <div className="layer-row" key={layer.name}>
                  <span className="layer-index">
                    {String(index + 1).padStart(2, "0")}
                  </span>
                  <div>
                    <p>{layer.label}</p>
                    <h3>{layer.name}</h3>
                    <span>{layer.detail}</span>
                  </div>
                  {index < layers.length - 1 ? (
                    <ArrowRight
                      className="layer-arrow"
                      size={20}
                      aria-hidden="true"
                    />
                  ) : (
                    <Layers3
                      className="layer-arrow final"
                      size={20}
                      aria-hidden="true"
                    />
                  )}
                </div>
              ))}
            </div>
          </div>
        </section>

        <section className="security-section">
          <div className="wrap security-inner">
            <ShieldCheck size={42} strokeWidth={1.5} aria-hidden="true" />
            <div>
              <p className="section-kicker dark">Credential boundary</p>
              <h2>Profiles are configuration. Secrets are not.</h2>
            </div>
            <p>
              Connection strings never enter <code>config.toml</code>. sift
              stores them in Windows Credential Manager, macOS Keychain, or the
              Linux Secret Service, with a session-only fallback.
            </p>
          </div>
        </section>

        <section className="start-section" id="start">
          <div className="wrap start-grid">
            <div>
              <p className="section-kicker">Run sift</p>
              <h2>Build the native app.</h2>
              <p>
                Rust 1.97 or newer is required. Windows and Linux are covered by
                the current CI matrix.
              </p>
              <div className="start-links">
                <a className="button button-dark" href={repository}>
                  <Code2 size={18} aria-hidden="true" />
                  View repository
                </a>
                <a
                  className="text-link dark-link"
                  href={`${repository}/blob/main/README.md`}
                >
                  Full setup guide
                  <ArrowRight size={18} aria-hidden="true" />
                </a>
              </div>
            </div>

            <div className="terminal" aria-label="Commands to build sift">
              <div className="terminal-bar">
                <Terminal size={17} aria-hidden="true" />
                <span>shell</span>
              </div>
              <pre>
                <code>
                  <span>git clone https://github.com/DeandreT/sift</span>
                  {"\n"}
                  <span>cd sift</span>
                  {"\n"}
                  <strong>cargo run -p sift</strong>
                </code>
              </pre>
            </div>
          </div>
        </section>
      </main>

      <footer>
        <div className="wrap footer-inner">
          <a className="brand footer-brand" href="#top">
            <span className="brand-mark small" aria-hidden="true">
              <i />
              <i />
              <i />
            </span>
            <span>sift</span>
          </a>
          <p>Azure Service Bus, in full view.</p>
          <a href={repository}>
            GitHub
            <ArrowRight size={16} aria-hidden="true" />
          </a>
        </div>
      </footer>
    </>
  );
}
