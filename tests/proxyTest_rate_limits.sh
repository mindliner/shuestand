#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${1:-https://shuestand.mountainlake.io}"
BURST="${2:-60}"
TIMEOUT="${TIMEOUT:-10}"

run_burst() {
  local label="$1"
  local method="$2"
  local url="$3"
  local body="${4:-}"
  local tmp
  tmp=$(mktemp)

  echo
  echo "== $label =="
  echo "${method} ${url} (burst=${BURST})"

  for _ in $(seq 1 "$BURST"); do
    if [[ -n "$body" ]]; then
      curl -sS -m "$TIMEOUT" -o /dev/null \
        -w "%{http_code} %{time_total}\n" \
        -X "$method" -H 'Content-Type: application/json' -d "$body" "$url" >>"$tmp" &
    else
      curl -sS -m "$TIMEOUT" -o /dev/null \
        -w "%{http_code} %{time_total}\n" \
        -X "$method" "$url" >>"$tmp" &
    fi
  done
  wait

  echo "Status distribution:"
  awk '{print $1}' "$tmp" | sort | uniq -c | sort -nr

  echo "Latency (s):"
  awk '{sum+=$2; if(min==0||$2<min)min=$2; if($2>max)max=$2} END {printf "avg=%.3f min=%.3f max=%.3f\n", sum/NR, min, max}' "$tmp"

  rm -f "$tmp"
}

echo "Base URL: $BASE_URL"
echo "Quick health check:"
curl -sS -i -m "$TIMEOUT" "$BASE_URL/healthz" | sed -n '1,12p'

run_burst "Read path (config)" "GET" "$BASE_URL/api/v1/config"
run_burst "Write path (sessions)" "POST" "$BASE_URL/api/v1/sessions" "{}"

echo
echo "Done. If limits are active, write-path bursts should show some 429 responses."
