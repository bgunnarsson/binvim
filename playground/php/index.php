<?php

declare(strict_types=1);

namespace Playground;

interface Greeter
{
    public function greet(string $name): string;
}

abstract class AbstractGreeter implements Greeter
{
    public function __construct(protected readonly string $prefix = 'Hello') {}

    public function greet(string $name): string
    {
        return sprintf('%s, %s!', $this->prefix, $name);
    }
}

final class FormalGreeter extends AbstractGreeter
{
}

enum Role: string
{
    case Admin = 'admin';
    case Editor = 'editor';
    case Viewer = 'viewer';

    public function label(): string
    {
        return match ($this) {
            self::Admin  => 'Administrator',
            self::Editor => 'Editor',
            self::Viewer => 'Viewer',
        };
    }
}

readonly class User
{
    public function __construct(
        public int $id,
        public string $name,
        public ?string $email = null,
        public Role $role = Role::Viewer,
    ) {}
}

$greeter = new FormalGreeter('Hi');

$users = [
    new User(1, 'Alice', 'alice@example.com', Role::Admin),
    new User(2, 'Bob'),
    new User(3, 'Carol', 'carol@example.com', Role::Editor),
];

foreach ($users as $user) {
    echo $greeter->greet($user->name) . " (role={$user->role->label()})\n";
}

$withEmail = array_filter($users, fn(User $u) => $u->email !== null);
echo 'with email: ' . count($withEmail) . PHP_EOL;
