#!/usr/bin/env python3
"""The figures of the experimental section, from ``results.json``.

Reads the aggregates ``make_results.py`` writes and renders the figures of
PDF, at a text width of 390 pt, so that they are
included at natural size and their type prints at the size it was set. The
experiments are still running while the figures are read; the update is

    python3 make_results.py && python3 make_figures.py

and the section text quotes numbers from the same ``results.json``, so figures
and prose move together or not at all.

Conventions, held across every figure so a reader carries the palette once:
color follows the configuration and never the chart, the search with both
bounds is blue, the model checker orange, the lower bound alone aqua, the upper
bound alone yellow, neither magenta; a sequential ramp is one hue; two series
that run close differ in dash as well as in hue; log axes wherever a decade
matters; no titles inside the figures, the caption being the title.
"""

from __future__ import annotations

import json
from pathlib import Path

import plotly.graph_objects as go
from plotly.subplots import make_subplots

HERE = Path(__file__).resolve().parent  # the experiment directory; everything generated lands here

from experiment_base import base

BASE = base(HERE)  # HERE, or the experiment named by --replay-experiment
FIGS = BASE / "figures"
R = json.loads((BASE / "results.json").read_text())

# the reference palette of the hub, light mode; identities are fixed here and
# reused identically in every figure
BOTH = "#2a78d6"      # the search, both bounds
STORM = "#eb6834"     # the model checker
LOWER = "#1baf7a"     # the lower bound alone
UPPER = "#eda100"     # the upper bound alone
NEITHER = "#e87ba4"   # neither bound
INK = "#0b0b0b"
MUTED = "#52514e"
GRID = "#e9e8e4"

FONT = dict(family="Times New Roman, serif", size=9, color=INK)
# above the panels, outside the plot area, so no box ever sits on a line
LEGEND_TOP = dict(orientation="h", x=0, y=1.16, xanchor="left", yanchor="bottom",
                  font=dict(size=8), bgcolor="rgba(0,0,0,0)")
AXIS = dict(showline=True, linecolor=MUTED, linewidth=0.7, ticks="outside",
            tickcolor=MUTED, tickfont=dict(size=8), gridcolor=GRID,
            gridwidth=0.5, zeroline=False, title_font=dict(size=9))


def layout(fig, w, h, **kw):
    fig.update_layout(width=w, height=h, font=FONT, plot_bgcolor="white",
                      paper_bgcolor="white",
                      margin=dict(l=44, r=10,
                                  t=26 if kw.get("showlegend") else 8,
                                  b=34), **kw)
    fig.update_xaxes(**AXIS)
    fig.update_yaxes(**AXIS)


def save(fig, name):
    out = FIGS / name
    # kaleido reads the layout in CSS pixels at 96 to the inch and writes the
    # page in points at 72, so a figure laid out at 390 must be exported at four
    # thirds for the page to come out at 390 pt, the target text width,
    # with its 9 pt type printing at 9 pt
    fig.write_image(str(out), format="pdf", scale=4 / 3)
    print(out.name)


def diag_rows(table):
    ds = sorted(int(d) for d in table)
    return ds, [table[str(d)] for d in ds]


def done_rows(table):
    """The rows of the diagonals the model checker has finished. A diagonal
    it is still working through is a biased sample of its cells, and its
    total would be read against the full total of the diagonal below it, so
    every figure of the comparison stops where the pass has finished. The
    flag is computed by make_results.py, so a diagonal joins the figures of
    its own accord."""
    ds = sorted(int(d) for d in table if table[d]["complete"])
    return ds, [table[str(d)] for d in ds]


# -- figure 1: the questions, in two panels --------------------------------
# (a) the share of questions refused by diagonal, the bite of the bisection.
# (b) the seconds the search and Storm spent on the questions both answered,
# by diagonal, side by side on a log axis, stopping at the last diagonal the
# pass has finished. The quadrant panel of who answered within the timeout
# was cut on 2026-08-25: with twelve timeouts of Storm and nothing else off
# the diagonal, four cells said what half a sentence of the text now says.


def fig_game():
    fig = make_subplots(cols=2, rows=1, horizontal_spacing=0.14,
                        column_widths=[0.5, 0.5])
    # left: the bite, by diagonal, over every question the search answered,
    # which covers the whole scope, the search having played every game
    ds_a = sorted(int(d) for d in R["leader"]["answered_by_diagonal"])
    pct = [100 * R["leader"]["no_by_diagonal"][str(d)]
           / R["leader"]["answered_by_diagonal"][str(d)] for d in ds_a]
    fig.add_trace(go.Bar(
        x=ds_a, y=pct, marker_color=BOTH, showlegend=False,
        cliponaxis=False), row=1, col=1)
    # right: the seconds of the two on the questions both answered, on the
    # finished diagonals, two bars side by side on a log axis
    ds, rows = done_rows(R["storm"]["by_diagonal"])
    fig.add_trace(go.Bar(
        x=ds, y=[max(r["paco_seconds"], 1e-3) for r in rows], name="search",
        marker_color=BOTH), row=1, col=2)
    fig.add_trace(go.Bar(
        x=ds, y=[max(r["storm_seconds"], 1e-3) for r in rows],
        name="Storm", marker_color=STORM), row=1, col=2)
    layout(fig, 390, 140, showlegend=False, bargap=0.3, bargroupgap=0.05,
           barmode="group")
    fig.update_xaxes(title_text="diagonal", dtick=1, showgrid=False,
                     tickangle=0, row=1, col=1)
    fig.update_yaxes(title_text="questions refused (%)",
                     range=[0, max(pct) * 1.1], row=1, col=1)
    fig.update_xaxes(title_text="diagonal", dtick=1, showgrid=False,
                     row=1, col=2)
    # the frame leaves a decade and a half of headroom above the tallest bar
    # for the key, and the ticks stop where the bars do
    import math
    top = math.log10(max(r["storm_seconds"] for r in rows))
    fig.update_yaxes(type="log", title_text="seconds, answered by both",
                     range=[0.5, top + 1.1],
                     tickvals=[10 ** k for k in range(1, int(top) + 1)],
                     row=1, col=2)
    # the key is two words in their own hue, in the headroom at the top left
    dom2 = fig.layout.xaxis2.domain
    for k, (name, color) in enumerate([("search", BOTH), ("Storm", STORM)]):
        fig.add_annotation(x=dom2[0] + 0.01, y=1.0, xref="paper",
                           yref="paper", text=name, showarrow=False,
                           xanchor="left", yanchor="top", yshift=-2 - 11 * k,
                           font=dict(size=8, color=color))
    # the panel letters, at the top left corner of each panel
    for i, letter in enumerate("ab"):
        dom = fig.layout[f"xaxis{i + 1 if i else ''}"].domain
        fig.add_annotation(x=dom[0], y=1.0, xref="paper", yref="paper",
                           text=f"({letter})", showarrow=False,
                           xanchor="right", yanchor="bottom", xshift=-2,
                           yshift=2, font=dict(size=8.5, color=INK))
    fig.update_xaxes(title_standoff=4)
    fig.update_yaxes(title_standoff=6)
    fig.update_layout(margin=dict(l=40, r=6, t=14, b=40))
    save(fig, "exp-game.pdf")


# -- figure 2: checker against search, per instance ------------------------
def fig_scatter():
    import math
    pts = R["storm"]["scatter"]
    ds = [p[0] for p in pts]
    xs = [max(p[1], 1e-4) for p in pts]
    ys = [max(p[2], 1e-3) for p in pts]
    # the frame follows the data, since the pass is still growing, but its
    # floor stays anchored low enough that the empty decades above the
    # equal-time guide, which are the finding, remain in view
    x0, x1 = math.log10(min(xs)) - 0.35, math.log10(max(xs)) + 0.35
    y0, y1 = math.log10(min(ys)) - 1.6, math.log10(max(ys)) + 0.3
    fig = go.Figure()
    pw, ph = 390 - 44 - 58, 300 - 8 - 34   # the plot area, for the label angle
    angle = -math.degrees(math.atan((ph / (y1 - y0)) / (pw / (x1 - x0))))
    for k, lab in [(1, "equal time"), (100, "100×"), (10000, "10 000×")]:
        fig.add_trace(go.Scatter(
            x=[10 ** (x0 - 1), 10 ** (x1 + 1)],
            y=[10 ** (x0 - 1) * k, 10 ** (x1 + 1) * k], mode="lines",
            line=dict(color="#d8d7d3", width=1, dash="dot"), showlegend=False))
        # the label sits on the guide, pulled inside the frame where the guide
        # leaves it
        lx = x0 + 0.12 * (x1 - x0)
        ly = lx + math.log10(k)
        if ly > y1 - 0.1:
            ly = y1 - 0.12
            lx = ly - math.log10(k)
        if ly < y0 + 0.1:
            ly = y0 + 0.3
            lx = ly - math.log10(k)
        fig.add_annotation(x=lx, y=ly, text=lab, showarrow=False,
                           textangle=angle, font=dict(size=7, color=MUTED),
                           yshift=7)
    fig.add_trace(go.Scatter(
        x=xs, y=ys, mode="markers",
        marker=dict(size=3.4, color=ds,
                    colorscale=[[0, "#9fc2e8"], [1, "#123a6b"]],
                    cmin=2, cmax=11, opacity=0.75,
                    colorbar=dict(title=dict(text="diagonal", font=dict(size=8)),
                                  thickness=7, len=0.85, tickfont=dict(size=8),
                                  dtick=1, outlinewidth=0)),
        showlegend=False))
    layout(fig, 390, 300)
    fig.update_xaxes(type="log", title_text="the search, seconds for the instance",
                     range=[x0, x1], dtick=1)
    fig.update_yaxes(type="log", title_text="the model checker, seconds",
                     range=[y0, y1], dtick=1)
    save(fig, "exp-checker-scatter.pdf")


# -- figure 3: the medians and the coverage, by diagonal --------------------
def fig_medians():
    ds, rows = done_rows(R["storm"]["by_diagonal"])
    fig = make_subplots(cols=2, rows=1, horizontal_spacing=0.16)
    fig.add_trace(go.Scatter(
        x=ds, y=[r["storm_round_median"] for r in rows], mode="lines+markers",
        name="model checker", line=dict(color=STORM, width=2),
        marker=dict(size=6)), row=1, col=1)
    fig.add_trace(go.Scatter(
        x=ds, y=[r["paco_round_median"] for r in rows], mode="lines+markers",
        name="search", line=dict(color=BOTH, width=2),
        marker=dict(size=6)), row=1, col=1)
    lost = [100 * r["unanswered_rounds"] /
            (r["rounds"] + r["unanswered_rounds"]) for r in rows]
    fig.add_trace(go.Bar(x=ds, y=lost, marker_color=STORM, showlegend=False,
                         text=[f"{v:.1f}" if v else "0" for v in lost],
                         textposition="outside", cliponaxis=False,
                         textfont=dict(size=7.5, color=MUTED)), row=1, col=2)
    layout(fig, 390, 205, showlegend=True, legend=LEGEND_TOP, bargap=0.4)
    fig.update_xaxes(title_text="diagonal", dtick=1, row=1, col=1)
    fig.update_xaxes(title_text="diagonal", dtick=1, showgrid=False, row=1, col=2)
    fig.update_yaxes(type="log", title_text="median seconds per question",
                     dtick=1, row=1, col=1)
    fig.update_yaxes(title_text="questions unanswered (%)", rangemode="tozero",
                     row=1, col=2)
    save(fig, "exp-round-medians.pdf")


# -- figure 4: the two bounds, four panels on one line ----------------------
# The ablation and the size of the strategy in one figure since 2026-08-25:
# (a) seconds for the diagonal, log; (b) instances not finished; (c) the mean
# decisions of the returned strategy, the two single-test configurations;
# (d) the share of questions on which it is strictly smaller with the
# accepting test. One palette carries the four panels: color follows the
# configuration, and the legend names each by the test it keeps, shortened
# to the symbol, the caption beneath the figure spelling the signatures out.
def fig_bounds():
    series = [("both", "\U0001d53c\u0302 and \U0001d543\u0302", BOTH, "solid"),
              ("lower", "\U0001d543\u0302 alone", LOWER, "dash"),
              ("upper", "\U0001d53c\u0302 alone", UPPER, "solid"),
              ("neither", "neither", NEITHER, "dash")]
    fig = make_subplots(cols=4, rows=1, horizontal_spacing=0.09)
    for key, name, color, dash in series:
        ds, rows = diag_rows(R["ablation"][key])
        fig.add_trace(go.Scatter(
            x=ds, y=[max(r["seconds"], 1) for r in rows], mode="lines+markers",
            name=name, line=dict(color=color, width=1.8, dash=dash),
            marker=dict(size=4)), row=1, col=1)
        fig.add_trace(go.Scatter(
            x=ds, y=[r["instances_lost"] for r in rows], mode="lines+markers",
            line=dict(color=color, width=1.8, dash=dash), marker=dict(size=4),
            showlegend=False), row=1, col=2)
    ds, rows = diag_rows(R["size"]["by_diagonal"])
    fig.add_trace(go.Scatter(
        x=ds, y=[r["mean_decisions_noupper"] for r in rows],
        mode="lines+markers", showlegend=False,
        line=dict(color=LOWER, width=1.8, dash="dash"), marker=dict(size=4)),
        row=1, col=3)
    fig.add_trace(go.Scatter(
        x=ds, y=[r["mean_decisions_upper"] for r in rows],
        mode="lines+markers", showlegend=False,
        line=dict(color=UPPER, width=2), marker=dict(size=4)), row=1, col=3)
    smaller = [100 * r["smaller"] / r["pairs"] for r in rows]
    fig.add_trace(go.Bar(x=ds, y=smaller, marker_color=UPPER,
                         showlegend=False), row=1, col=4)
    legend = dict(LEGEND_TOP, y=1.06, font=dict(size=7.5))
    layout(fig, 390, 165, showlegend=True, legend=legend, bargap=0.35)
    fig.update_xaxes(title_text="diagonal", dtick=2, tick0=2, showgrid=False,
                     tickangle=0)
    fig.update_yaxes(type="log", title_text="seconds", dtick=1, row=1, col=1)
    fig.update_yaxes(title_text="instances not finished", rangemode="tozero",
                     row=1, col=2)
    fig.update_yaxes(title_text="decisions, mean", rangemode="tozero",
                     row=1, col=3)
    fig.update_yaxes(title_text="strictly smaller (%)", range=[0, 68],
                     row=1, col=4)
    for i, letter in enumerate("abcd"):
        dom = fig.layout[f"xaxis{i + 1 if i else ''}"].domain
        fig.add_annotation(x=dom[0], y=1.0, xref="paper", yref="paper",
                           text=f"({letter})", showarrow=False,
                           xanchor="right", yanchor="bottom", xshift=-2,
                           yshift=2, font=dict(size=8.5, color=INK))
    fig.update_xaxes(title_standoff=4)
    fig.update_yaxes(title_standoff=4)
    fig.update_layout(margin=dict(l=34, r=6, t=30, b=40))
    save(fig, "exp-bounds.pdf")


if __name__ == "__main__":
    FIGS.mkdir(exist_ok=True)
    fig_game()
    fig_scatter()
    fig_medians()
    fig_bounds()
