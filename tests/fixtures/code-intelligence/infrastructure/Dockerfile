FROM rust:1 AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin api

FROM scratch AS runtime
COPY --from=builder /build/target/release/api /app/api
ENTRYPOINT ["/app/api"]
