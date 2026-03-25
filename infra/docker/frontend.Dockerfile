# syntax=docker/dockerfile:1
FROM node:20 AS builder
WORKDIR /app
COPY frontend/package*.json ./
RUN npm install
COPY frontend ./
ARG VITE_SHUESTAND_API_BASE=
ENV VITE_SHUESTAND_API_BASE=$VITE_SHUESTAND_API_BASE
RUN npm run build

FROM nginx:alpine
COPY infra/docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=builder /app/dist /usr/share/nginx/html
EXPOSE 80
