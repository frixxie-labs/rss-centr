FROM docker.io/denoland/deno:latest

WORKDIR /app

COPY . .

RUN deno install --allow-scripts

RUN deno task build

EXPOSE 8000

CMD ["serve", "-A", "_fresh/server.js"]
