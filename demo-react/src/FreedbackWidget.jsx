import { useEffect, useRef } from "react";

// Renders one of the shipped Freedback custom elements (<freedback-stars> etc.)
// from React, WITHOUT rebuilding them.
//
// The load-bearing custom-element-in-React gotcha this wrapper exists to solve:
//
//   The widget's lifecycle is connectedCallback -> render() + refresh(), where
//   render() reads data-publish to decide whether to wire up (enabled) submit
//   buttons, and refresh() reads data-read/data-target to fetch the aggregate.
//   ALL of that runs the moment the element is connected to the document.
//
//   If we render <freedback-stars ref={...}> from JSX, React INSERTS the node
//   into the DOM first and runs the ref callback AFTER. So connectedCallback
//   would fire with data-target/data-read/data-publish still unset — refresh()
//   early-returns (no aggregate) and the publish buttons render disabled. Setting
//   the attributes from the ref afterward does NOT re-run render()/refresh().
//
//   Fix: do not let React create/connect the element. Render a plain host <span>,
//   then imperatively create the custom element, set ALL its attributes while it
//   is still DETACHED, and only then append it. connectedCallback now fires with
//   every attribute already in place — identical to writing the tag in static
//   HTML. data-sign is a valueless boolean attribute (the widget checks
//   hasAttribute), so we set it to "" when enabled and never to "false".
export default function FreedbackWidget({ kind, target, read, publish, sign, token, license, ...rest }) {
  const hostRef = useRef(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const el = document.createElement(kind);
    if (target != null) el.setAttribute("data-target", target);
    if (read != null) el.setAttribute("data-read", read);
    if (publish != null) el.setAttribute("data-publish", publish);
    if (sign) el.setAttribute("data-sign", ""); // present == enabled (boolean attr)
    if (token != null) el.setAttribute("data-token", token);
    if (license != null) el.setAttribute("data-license", license); // rights IRI (ADR 0022)
    // Pass through extra data-* config (e.g. data-worst/data-best/data-step).
    for (const [k, v] of Object.entries(rest)) {
      if (k.startsWith("data-") && v != null) el.setAttribute(k, String(v));
    }
    // Attributes are all set on the detached element; appending now connects it,
    // so connectedCallback (render + refresh) sees the full configuration.
    host.appendChild(el);
    return () => {
      // Clean up on unmount / prop change so we don't accumulate duplicates.
      if (el.parentNode === host) host.removeChild(el);
    };
    // Re-create the element if any configuration prop changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [kind, target, read, publish, sign, token, license, JSON.stringify(rest)]);

  return <span ref={hostRef} className="fb-host" />;
}
