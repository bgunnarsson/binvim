import { useState } from "react";

export function Counter({ initial = 0, step = 1 }) {
    const [count, setCount] = useState(initial);

    return (
        <div className="counter">
            <button onClick={() => setCount((c) => c - step)}>-</button>
            <span>{count}</span>
            <button onClick={() => setCount((c) => c + step)}>+</button>
        </div>
    );
}

export default function App() {
    return (
        <main>
            <Counter initial={10} step={2} />
            <Counter />
        </main>
    );
}
