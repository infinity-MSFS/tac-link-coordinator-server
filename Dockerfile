# Dockerfile
FROM rust:1.85 as builder
WORKDIR /usr/src/app

# Cache dependencies by copying manifests first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && printf "fn main() {println!(\"dummy\");}" > src/main.rs

# Fetch dependencies
RUN cargo fetch

# Copy real source and build
COPY src ./src
RUN cargo install --path . --root /usr/local/cargo

# Runtime image with newer glibc (provides GLIBC_2.33+)
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /usr/local/cargo/bin/ /usr/local/bin/

# Non-root user
RUN useradd -m -u 1000 app || true
USER 1000

EXPOSE 3000
ENV RUST_LOG=info

# If the actual binary name differs, replace CoordinatorServer with your binary name
CMD ["CoordinatorServer"]