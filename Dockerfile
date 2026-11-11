# ---- build ----
FROM rust:1.88-bookworm AS build
WORKDIR /app
# Cache dependencies separately from the app source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src
COPY src ./src
COPY static ./static
RUN touch src/main.rs && cargo build --release

# ---- run ----
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 planner
WORKDIR /app
COPY --from=build /app/target/release/phuse-planner /usr/local/bin/phuse-planner
COPY --chown=planner data ./data
USER planner
ENV PORT=8080 DATA_PATH=/app/data/sessions.json
EXPOSE 8080
CMD ["phuse-planner"]
