// Simple in-memory event bus + a couple of consumers.
const bus = (() => {
    const listeners = new Map();

    return {
        on(event, fn) {
            if (!listeners.has(event)) listeners.set(event, new Set());
            listeners.get(event).add(fn);
            return () => listeners.get(event).delete(fn);
        },
        emit(event, payload) {
            const set = listeners.get(event);
            if (!set) return;
            for (const fn of set) fn(payload);
        },
    };
})();

class Logger {
    constructor(prefix = "log") {
        this.prefix = prefix;
    }

    info(msg) {
        console.log(`[${this.prefix}] ${msg}`);
    }

    error(msg) {
        console.error(`[${this.prefix}!] ${msg}`);
    }
}

const log = new Logger("bus");

bus.on("user:created", (u) => log.info(`new user ${u.name}`));
bus.on("user:created", async (u) => {
    await new Promise((r) => setTimeout(r, 10));
    log.info(`welcome email queued for ${u.email}`);
});

const users = [
    { id: 1, name: "Alice", email: "alice@example.com" },
    { id: 2, name: "Bob",   email: "bob@example.com" },
];

for (const u of users) {
    bus.emit("user:created", u);
}

const total = users.reduce((sum, u) => sum + u.id, 0);
log.info(`total ids = ${total}`);

export { bus, Logger };
