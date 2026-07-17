(() => {
  function sourceAnchors() {
    const anchors = Array.from(document.querySelectorAll(".source-anchor[data-source-start]"))
      .map((element) => ({
        element,
        offset: Number.parseInt(element.dataset.sourceStart, 10),
        y: element.getBoundingClientRect().top + window.scrollY,
      }))
      .filter((anchor) => Number.isFinite(anchor.offset))
      .sort((left, right) => left.offset - right.offset || left.y - right.y);

    return anchors.reduce((unique, anchor) => {
      const previous = unique[unique.length - 1];
      if (previous?.offset === anchor.offset) {
        previous.y = Math.min(previous.y, anchor.y);
      } else {
        unique.push(anchor);
      }
      return unique;
    }, []);
  }

  function clamp(value, min, max) {
    return Math.min(max, Math.max(min, value));
  }

  const maxOvershootDistance = 100;
  const overshootDecay = 0.80;
  const overshootEdges = ["top", "bottom", "left", "right"];
  const overshootOpposite = {
    top: "bottom",
    bottom: "top",
    left: "right",
    right: "left",
  };
  const overshoot = {
    top: 0,
    bottom: 0,
    left: 0,
    right: 0,
  };
  let overshootNodes = null;
  let overshootAnimationFrame = 0;
  let previousTouchPoint = null;

  function ensureOvershootNodes() {
    if (overshootNodes) return overshootNodes;

    overshootNodes = {};
    for (const edge of overshootEdges) {
      const node = document.createElement("div");
      node.className = `craic-overshoot craic-overshoot-${edge}`;
      node.setAttribute("aria-hidden", "true");
      overshootNodes[edge] = node;
      (document.body || document.documentElement).appendChild(node);
    }

    return overshootNodes;
  }

  function updateOvershootNodes() {
    const nodes = ensureOvershootNodes();
    nodes.top.style.height = `${overshoot.top}px`;
    nodes.bottom.style.height = `${overshoot.bottom}px`;
    nodes.left.style.width = `${overshoot.left}px`;
    nodes.right.style.width = `${overshoot.right}px`;

    for (const edge of overshootEdges) {
      nodes[edge].style.opacity = overshoot[edge] > 0.5 ? "1" : "0";
    }
  }

  function animateOvershoot() {
    overshootAnimationFrame = 0;
    let active = false;

    for (const edge of overshootEdges) {
      const next = overshoot[edge] * overshootDecay;
      if (next < 0.5) {
        overshoot[edge] = 0;
      } else {
        overshoot[edge] = next;
        active = true;
      }
    }

    updateOvershootNodes();
    if (active) overshootAnimationFrame = window.requestAnimationFrame(animateOvershoot);
  }

  function queueOvershootDecay() {
    if (!overshootAnimationFrame) {
      overshootAnimationFrame = window.requestAnimationFrame(animateOvershoot);
    }
  }

  function pullOvershoot(edge, overflow) {
    if (!Number.isFinite(overflow) || overflow <= 0) return;

    overshoot[overshootOpposite[edge]] = 0;
    overshoot[edge] = clamp(overshoot[edge] + Math.abs(overflow), 0, maxOvershootDistance);
    updateOvershootNodes();
    queueOvershootDecay();
  }

  function scrollMetrics() {
    const scroller = document.scrollingElement || document.documentElement;
    return {
      x: window.scrollX,
      y: window.scrollY,
      maxX: Math.max(0, scroller.scrollWidth - window.innerWidth),
      maxY: Math.max(0, scroller.scrollHeight - window.innerHeight),
    };
  }

  function pullOvershootForDelta(deltaX, deltaY) {
    if (Math.abs(deltaX) <= Number.EPSILON && Math.abs(deltaY) <= Number.EPSILON) return;

    const metrics = scrollMetrics();
    const desiredX = metrics.x + deltaX;
    const desiredY = metrics.y + deltaY;

    if (metrics.maxX > Number.EPSILON && desiredX < 0) {
      pullOvershoot("left", -desiredX);
    } else if (metrics.maxX > Number.EPSILON && desiredX > metrics.maxX) {
      pullOvershoot("right", desiredX - metrics.maxX);
    }

    if (metrics.maxY > Number.EPSILON && desiredY < 0) {
      pullOvershoot("top", -desiredY);
    } else if (metrics.maxY > Number.EPSILON && desiredY > metrics.maxY) {
      pullOvershoot("bottom", desiredY - metrics.maxY);
    }
  }

  function wheelDeltaPixels(event) {
    let multiplier = 1;
    if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
      const lineHeight = Number.parseFloat(window.getComputedStyle(document.body).lineHeight);
      multiplier = Number.isFinite(lineHeight) ? lineHeight : 16;
    } else if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
      multiplier = Math.max(window.innerHeight, 1);
    }

    return {
      x: event.deltaX * multiplier,
      y: event.deltaY * multiplier,
    };
  }

  function installOvershootHandlers() {
    ensureOvershootNodes();

    window.addEventListener("wheel", (event) => {
      const delta = wheelDeltaPixels(event);
      pullOvershootForDelta(delta.x, delta.y);
    }, { passive: true });

    window.addEventListener("touchstart", (event) => {
      if (event.touches.length !== 1) {
        previousTouchPoint = null;
        return;
      }

      const touch = event.touches[0];
      previousTouchPoint = { x: touch.clientX, y: touch.clientY };
    }, { passive: true });

    window.addEventListener("touchmove", (event) => {
      if (event.touches.length !== 1 || previousTouchPoint === null) return;

      const touch = event.touches[0];
      const deltaX = previousTouchPoint.x - touch.clientX;
      const deltaY = previousTouchPoint.y - touch.clientY;
      previousTouchPoint = { x: touch.clientX, y: touch.clientY };
      pullOvershootForDelta(deltaX, deltaY);
    }, { passive: true });

    window.addEventListener("touchend", () => {
      previousTouchPoint = null;
    }, { passive: true });

    window.addEventListener("touchcancel", () => {
      previousTouchPoint = null;
    }, { passive: true });
  }

  function yForSourceOffset(offset) {
    const anchors = sourceAnchors();
    if (anchors.length === 0) return null;

    const target = Number(offset);
    if (!Number.isFinite(target) || target <= anchors[0].offset) return anchors[0].y;

    for (let index = 0; index + 1 < anchors.length; index += 1) {
      const current = anchors[index];
      const next = anchors[index + 1];
      if (target > next.offset) continue;

      const sourceSpan = Math.max(1, next.offset - current.offset);
      const progress = clamp((target - current.offset) / sourceSpan, 0, 1);
      return current.y + (next.y - current.y) * progress;
    }

    return anchors[anchors.length - 1].y;
  }

  function sourceOffsetForY(y) {
    const anchors = sourceAnchors();
    if (anchors.length === 0) return null;

    const target = Number(y);
    if (!Number.isFinite(target) || target <= anchors[0].y) return anchors[0].offset;

    for (let index = 0; index + 1 < anchors.length; index += 1) {
      const current = anchors[index];
      const next = anchors[index + 1];
      if (target > next.y) continue;

      const visualSpan = Math.max(1, next.y - current.y);
      const progress = clamp((target - current.y) / visualSpan, 0, 1);
      return Math.round(current.offset + (next.offset - current.offset) * progress);
    }

    return anchors[anchors.length - 1].offset;
  }

  function scrollToSourceOffset(offset) {
    const y = yForSourceOffset(offset);
    if (y === null) return false;

    const maxY = Math.max(0, document.documentElement.scrollHeight - window.innerHeight);
    window.scrollTo({ top: clamp(y, 0, maxY), behavior: "auto" });
    postSourceOffset();
    return true;
  }

  function sourceOffsetAtViewportTop() {
    return sourceOffsetForY(window.scrollY);
  }

  window.CraicMarkdownPreview = {
    scrollToSourceOffset,
    sourceOffsetAtViewportTop,
    reportSourceOffset: postSourceOffset,
  };
  window.scrollToSourceOffset = scrollToSourceOffset;
  window.sourceOffsetAtViewportTop = sourceOffsetAtViewportTop;

  let pendingPost = 0;
  function postSourceOffset() {
    if (pendingPost) window.cancelAnimationFrame(pendingPost);
    pendingPost = window.requestAnimationFrame(() => {
      pendingPost = 0;
      const offset = sourceOffsetAtViewportTop();
      const handler = window.webkit?.messageHandlers?.sourceOffsetChanged;
      if (offset !== null && handler) handler.postMessage(offset);
    });
  }

  window.addEventListener("scroll", postSourceOffset, { passive: true });
  window.addEventListener("resize", postSourceOffset);
  window.addEventListener("load", postSourceOffset);
  document.addEventListener("DOMContentLoaded", postSourceOffset);
  document.addEventListener("click", (event) => {
    const link = event.target?.closest?.("a[href]");
    if (!link) return;

    event.preventDefault();
    event.stopPropagation();
    const handler = window.webkit?.messageHandlers?.openMarkdownLink;
    if (handler) handler.postMessage(link.href);
  }, true);
  installOvershootHandlers();
  postSourceOffset();
})();
