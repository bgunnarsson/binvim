-- Sample schema + queries for a tiny e-commerce DB.

CREATE TABLE users (
    id          SERIAL PRIMARY KEY,
    email       VARCHAR(255) NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    is_active   BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE INDEX users_email_idx ON users (email);

CREATE TABLE products (
    id         SERIAL PRIMARY KEY,
    sku        VARCHAR(64) NOT NULL UNIQUE,
    name       TEXT NOT NULL,
    price_cents INTEGER NOT NULL CHECK (price_cents >= 0)
);

CREATE TABLE orders (
    id          SERIAL PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    placed_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status      VARCHAR(16) NOT NULL DEFAULT 'pending'
);

CREATE TABLE order_items (
    order_id    INTEGER NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    product_id  INTEGER NOT NULL REFERENCES products(id),
    quantity    INTEGER NOT NULL CHECK (quantity > 0),
    PRIMARY KEY (order_id, product_id)
);

-- Seed
INSERT INTO users (email, name) VALUES
    ('alice@example.com', 'Alice'),
    ('bob@example.com',   'Bob');

INSERT INTO products (sku, name, price_cents) VALUES
    ('SKU-1', 'Notebook', 1599),
    ('SKU-2', 'Pen',       299),
    ('SKU-3', 'Eraser',    149);

-- Top customers by spend in the last 30 days.
SELECT  u.id,
        u.name,
        SUM(p.price_cents * oi.quantity) AS spent_cents,
        COUNT(DISTINCT o.id)             AS order_count
FROM users u
JOIN orders o      ON o.user_id    = u.id
JOIN order_items oi ON oi.order_id = o.id
JOIN products p     ON p.id        = oi.product_id
WHERE   o.placed_at >= NOW() - INTERVAL '30 days'
    AND o.status      <> 'cancelled'
GROUP BY u.id, u.name
HAVING  SUM(p.price_cents * oi.quantity) > 10000
ORDER BY spent_cents DESC
LIMIT 10;
