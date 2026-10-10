import { Fragment, type ReactNode } from "react";

/** Simple, safe Markdown renderer (headings, lists, bold, blockquote) — no innerHTML. */
function inline(text: string): ReactNode[] {
  const parts = text.split(/(\*\*[^*]+\*\*)/g);
  return parts.map((p, i) =>
    p.startsWith("**") && p.endsWith("**") && p.length > 4 ? <strong key={i}>{p.slice(2, -2)}</strong> : <Fragment key={i}>{p}</Fragment>,
  );
}

export default function Markdown({ text }: { text: string }) {
  const lines = text.split("\n");
  const out: ReactNode[] = [];
  let list: ReactNode[] = [];
  const flush = () => {
    if (list.length) {
      out.push(<ul key={`ul${out.length}`}>{list}</ul>);
      list = [];
    }
  };
  lines.forEach((raw, i) => {
    const line = raw.trimEnd();
    const bullet = line.match(/^\s*(?:[-*]|\d+\.)\s+(.*)$/);
    if (bullet) {
      // Task list from the summary: "- [ ] to do…" / "- [x] done".
      const task = bullet[1].match(/^\[( |x|X)\]\s+(.*)$/);
      list.push(
        task ? (
          <li key={i} className="task">
            <span className={`md-check${task[1] === " " ? "" : " done"}`} aria-hidden />
            {inline(task[2])}
          </li>
        ) : (
          <li key={i}>{inline(bullet[1])}</li>
        ),
      );
      return;
    }
    flush();
    if (!line.trim()) return;
    const h = line.match(/^(#{1,4})\s+(.*)$/);
    if (h) {
      const level = h[1].length;
      const content = inline(h[2]);
      out.push(level === 1 ? <h2 key={i}>{content}</h2> : level === 2 ? <h3 key={i}>{content}</h3> : <h4 key={i}>{content}</h4>);
    } else if (line.startsWith(">")) {
      out.push(
        <blockquote key={i} className="md-quote">
          {inline(line.replace(/^>\s?/, ""))}
        </blockquote>,
      );
    } else {
      out.push(<p key={i}>{inline(line)}</p>);
    }
  });
  flush();
  return <div className="markdown">{out}</div>;
}
