"""A small Python sample exercising classes, decorators, async, and types."""

from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from typing import Callable, Iterable, Protocol


class Greeter(Protocol):
    def greet(self, name: str) -> str: ...


@dataclass
class User:
    id: int
    name: str
    email: str | None = None
    tags: list[str] = field(default_factory=list)


def timed[F: Callable](fn: F) -> F:
    import time

    def wrapper(*args, **kwargs):
        start = time.perf_counter()
        try:
            return fn(*args, **kwargs)
        finally:
            elapsed = time.perf_counter() - start
            print(f"{fn.__name__} took {elapsed * 1000:.2f}ms")

    return wrapper  # type: ignore[return-value]


class FormalGreeter:
    def __init__(self, prefix: str = "Hello") -> None:
        self._prefix = prefix

    def greet(self, name: str) -> str:
        return f"{self._prefix}, {name}!"


@timed
def summarise(users: Iterable[User]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for u in users:
        for tag in u.tags or ["untagged"]:
            counts[tag] = counts.get(tag, 0) + 1
    return counts


async def fetch_user(uid: int) -> User:
    await asyncio.sleep(0)
    return User(id=uid, name=f"user-{uid}", tags=["alpha", "beta"])


async def main() -> None:
    users = await asyncio.gather(*(fetch_user(i) for i in range(3)))
    greeter: Greeter = FormalGreeter("Hi")
    for u in users:
        print(greeter.greet(u.name))
    print(summarise(users))


if __name__ == "__main__":
    asyncio.run(main())
