import { useState, useEffect, useCallback } from "react";

type Todo = {
    id: number;
    text: string;
    done: boolean;
};

type TodoItemProps = {
    todo: Todo;
    onToggle: (id: number) => void;
};

function TodoItem({ todo, onToggle }: TodoItemProps) {
    return (
        <li className={todo.done ? "todo done" : "todo"}>
            <input
                type="checkbox"
                checked={todo.done}
                onChange={() => onToggle(todo.id)}
            />
            <span>{todo.text}</span>
        </li>
    );
}

export default function App() {
    const [todos, setTodos] = useState<Todo[]>([
        { id: 1, text: "Write tests", done: false },
        { id: 2, text: "Read the docs", done: true },
    ]);
    const [draft, setDraft] = useState("");

    useEffect(() => {
        document.title = `${todos.filter((t) => !t.done).length} open`;
    }, [todos]);

    const toggle = useCallback((id: number) => {
        setTodos((prev) =>
            prev.map((t) => (t.id === id ? { ...t, done: !t.done } : t))
        );
    }, []);

    const add = () => {
        if (!draft.trim()) return;
        setTodos((prev) => [
            ...prev,
            { id: Date.now(), text: draft.trim(), done: false },
        ]);
        setDraft("");
    };

    return (
        <main className="app">
            <h1>Todos</h1>
            <ul>
                {todos.map((t) => (
                    <TodoItem key={t.id} todo={t} onToggle={toggle} />
                ))}
            </ul>
            <form onSubmit={(e) => { e.preventDefault(); add(); }}>
                <input
                    value={draft}
                    onChange={(e) => setDraft(e.target.value)}
                    placeholder="New todo…"
                />
                <button type="submit">Add</button>
            </form>
        </main>
    );
}
