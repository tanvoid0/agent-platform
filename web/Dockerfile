FROM node:22-bookworm-slim

RUN corepack enable && corepack prepare pnpm@10 --activate

WORKDIR /app

COPY package.json pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY . .

EXPOSE 3333

CMD ["pnpm", "exec", "vite", "--host", "0.0.0.0", "--port", "3333"]
