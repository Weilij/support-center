# Build the SPA, serve the static bundle with nginx proxying /api.
FROM node:22-slim AS builder
WORKDIR /app
COPY package.json package-lock.json ./
# npm install instead of npm ci: lockfiles written by npm 11 on macOS can
# mark platform-specific esbuild binaries non-optional, breaking ci on linux.
RUN npm install --no-fund --no-audit
COPY tsconfig.json vite.config.ts index.html ./
COPY src ./src
RUN npm run build

FROM nginx:1.27-alpine
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
