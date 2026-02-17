FROM denoland/deno:latest AS builder

WORKDIR /app

# Copy dependency files first for caching
COPY deno.json deno.lock ./

# Cache dependencies
RUN deno install

# Copy the rest of the source code
COPY . .

# Build the Fresh app
RUN deno task build

# --- Runtime stage ---
FROM denoland/deno:latest

WORKDIR /app

COPY --from=builder /app/_fresh/ ./_fresh/
COPY --from=builder /app/deno.json /app/deno.lock ./
COPY --from=builder /app/node_modules/ ./node_modules/
COPY --from=builder /app/static/ ./static/

EXPOSE 8000

CMD ["serve", "-A", "_fresh/server.js"]
