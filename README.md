# Shuestand

Shuestand lets users fund a Cashu wallet directly with on-chain Bitcoin or withdraw cashu sats back out, all through a single, kiosk-friendly interface.

## Why Shuestand?
Lightning swaps and do-it-yourself Cashu flows already exist with boltz.exchange, but not everybody who likes Bitplane’s promise wants to juggle routing nodes, mint swaps, or channel policy. Shuestand formalizes a simple dual strategy: save in hardened self-custody, spend from a lightweight privacy wallet, and let the bridge disappear into the background.

- **On-chain for large balances.** Big funds live in a multi-sig, time-locked, well-backed-up wallet where security trumps convenience and transaction counts stay low.
- **Cashu for day-to-day balances.** Smaller, high-churn funds sit in a privacy-preserving custodial wallet where “it just works” matters more than ceremony.

Shuestand ties those two halves together. It keeps Lightning behind the curtain, automates the Bitcoin ⇄ Cashu float management, and lets operators offer a single kiosk flow that delivers both the safety of a cold-savings mindset and the tap-and-go feel of ecash.

## User Flows
1. **Bitcoin → Cashu**
   - User requests a deposit, gets a unique on-chain address (BIP21 URI + QR).
   - Chain watcher tracks mempool + confirmation depth; status is streamed to the UI with confirmation countdowns.
   - Once the target confirmations clear, the backend mints tokens via the default mint. If the `delivery_hint` is an `http(s)` webhook we auto-post the token there; otherwise the kiosk shows a pickup screen (with resume code + clipboard helper) so the guest/operator can claim it later.

2. **Cashu → Bitcoin**
   - User pastes/imports a Cashu token, sees a quote (amount, projected miner fee, eta).
   - Funds are redeemed at the mint, credited to our float, and the backend crafts/broadcasts an on-chain payout from the hot wallet (no Lightning dependency).
   - UI shows progress, payout txid + confirmations, and an audit trail for operators.

### Cross-mint swaps
- Foreign Cashu tokens are imported into per-mint CDK wallets, melted via Lightning to the kiosk mint, and the resulting proofs are minted before we touch the on-chain payout.
- The swapper fetches the melt quote upfront, checks `invoice + fee_reserve + input_fee` against the wallet’s spendable proofs, and shrinks the invoice before any proofs are reserved so we don't burn float chasing an impossible payment.
- `MintQuoteState::Paid` is treated as success, so as soon as CLN marks the kiosk invoice paid we pull the canonical proofs and move the withdrawal to the on-chain stage.

## Installation

### Docker Deployment

```
git clone https://github.com/mindliner/shuestand.git
cd infra/docker/
cp backend.env.example backend.env
# edit backend.env
docker compose -p shuestand up -d --build
```

Now visit `http://localhost:8080` for the kiosk/operator UI; `/api` requests are reverse-proxied to the backend container.

Once running, update `infra/docker/backend.env` whenever you rotate keys/policies and restart the backend service (`docker compose restart backend`).

### Build From Source

```
# start backend first
git clone https://github.com/mindliner/shuestand.git
cd shuestand/backend
cp .env.example .your-env
# edit your-env, then source it (for example: "set -a; source your-env; set +a")
cargo run --bin backend # (or --bin wallet)

# start frontend
cd shuestand/frontend
npm run dev # run "npm install" once before
```

### Publishing behind a reverse proxy
When the Docker stack lives on an internal host (e.g., `vm-docker:8872`), expose it through a public reverse proxy so visitors reach it via HTTPS without touching the internal network.

1. **Pick the public domain + certificate.** Request a Let’s Encrypt cert (webroot or DNS) for the hostname you plan to expose.
2. **Create an upstream + redirect block.** On the proxy host (nginx example), add a server that forwards both the kiosk assets and `/api` calls to the compose frontend listener:

   ```nginx
   upstream shuestand_frontend {
       server vm-docker:8872;
   }

   server {
       listen 80;
       listen [::]:80;
       server_name example.domain.com;

       location /.well-known/acme-challenge/ {
           root /var/www/certbot;
       }

       return 301 https://$host$request_uri;
   }

   server {
       listen 443 ssl;
       listen [::]:443 ssl;
       server_name example.domain.com;

       ssl_certificate     /etc/letsencrypt/live/example.domain.com/fullchain.pem;
       ssl_certificate_key /etc/letsencrypt/live/example.domain.com/privkey.pem;
       include /etc/letsencrypt/options-ssl-nginx.conf;
       ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;

       location / {
           proxy_pass http://shuestand_frontend;
           include /etc/nginx/snippets/ssl-proxy-params.conf; # sets Host/X-Forwarded-* headers
       }
   }
   ```

3. **Reload nginx and verify HTTPS.** Visit `https://<domain>` and confirm that the kiosk loads and that the Nut18 QR transports point to the HTTPS host (the backend derives the callback URL from `Host` + `X-Forwarded-Proto`). Avoid double-proxy stacks that overwrite `X-Forwarded-Proto` with `http`, otherwise Cashu wallets will refuse to POST back to the funding endpoint.

