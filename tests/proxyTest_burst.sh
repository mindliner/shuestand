# normal health check should work
curl -i https://shuestand.mountainlake.io/healthz

# burst check, some requests should become 429
for i in {1..30}; do
  curl -s -o /dev/null -w "%{http_code}\n" https://shuestand.mountainlake.io/api/v1/config &
done
wait
