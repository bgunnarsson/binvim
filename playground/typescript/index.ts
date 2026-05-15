interface User {
    id: number;
    name: string;
    email?: string;
    roles: ReadonlyArray<Role>;
}

type Role = "admin" | "editor" | "viewer";

enum Status {
    Active = "active",
    Pending = "pending",
    Banned = "banned",
}

class UserService {
    private users: Map<number, User> = new Map();

    constructor(private readonly defaultRole: Role = "viewer") {}

    public add(user: Omit<User, "roles"> & { roles?: Role[] }): User {
        const full: User = {
            ...user,
            roles: user.roles ?? [this.defaultRole],
        };
        this.users.set(full.id, full);
        return full;
    }

    public async findById(id: number): Promise<User | null> {
        await new Promise((r) => setTimeout(r, 0));
        return this.users.get(id) ?? null;
    }

    public byRole(role: Role): User[] {
        return [...this.users.values()].filter((u) => u.roles.includes(role));
    }
}

function makeGreeter<T extends { name: string }>(prefix: string) {
    return (entity: T): string => `${prefix}, ${entity.name}!`;
}

const svc = new UserService("admin");
const u = svc.add({ id: 1, name: "Alice", email: "alice@example.com" });
const greet = makeGreeter<User>("Hello");

(async () => {
    const found = await svc.findById(u.id);
    if (found) {
        console.log(greet(found), `status=${Status.Active}`);
    }
})();
