# Recovery signal model

This document describes the decoder's local, decision-directed recovery models in
[`src/recovery_signal.rs`](../src/recovery_signal.rs). These are implementation
choices, including their thresholds and iteration limits. The symbol alphabet,
Gray mapping, cell layout, and pilot calibration are specified in
[`FORMAT.md`](../FORMAT.md); recovery introduces no additional payload dimensions.

## Inputs and stages

A cell contains four quadrant luminance ranks and two shared chroma bits. For
quadrant $q\in\{0,1,2,3\}$, the two stored Gray bits are
$g_q=(s\mathbin{\gg}2q)\mathbin{\&}3$, and the rank is
$r_q=g_q\mathbin{\oplus}(g_q\mathbin{\gg}1)$. The emitted alphabet is
$Y_q=128+28(2r_q-3)$. Shared chroma signs are
$z_b=2((s\mathbin{\gg}8)\mathbin{\&}1)-1$ and
$z_r=2((s\mathbin{\gg}9)\mathbin{\&}1)-1$.

Blind initialization proceeds through these stages:

1. Estimate a local four-level luminance alphabet and two-level chroma alphabets
   from `raw` features, then classify each cell.
2. Refine luminance through `luma_channel` using `raw` features.
3. Fit the binary chroma models below using `raw` features.
4. Fit the coupled luminance model below using the first four `mixed` features.

`raw` and `mixed` are two feature representations supplied by the recovery
caller; each has twelve slots, of which these stages use the first six or four.
The scanner supplies aligned lattice measurements in `raw`; `mixed` substitutes
spatially equalized quadrant luminance and retains the same shared chroma
measurements. The seed and binary chroma stages do not use the last six slots. The data mask excludes reserved cells and
other unavailable data. The coupled stage keeps the chroma decisions fixed at its
input while updating quadrant ranks.

The module boundaries follow these stages: `initial.rs` constructs the seed,
`levels.rs` estimates its luminance field, `chroma.rs` fits binary chroma, and
`coupled/{field,model,inference}.rs` construct local supports, fit coefficients,
and classify quadrants respectively. Tests remain private children of their
owning modules, with source files under `src/tests`.

## Binary chroma model

For each chroma axis independently, let $z_c\in\{-1,+1\}$ be the current sign at
cell $c$. With neighbors ordered up, down, left, right, the eight-entry basis is

$$
\phi_c=[1,z_c,z_U,z_D,z_L,z_R,m_c,h_c]^\mathsf T,
\qquad
m_c=\frac14\sum_q r_{c,q}-1.5,
\qquad
h_c=\frac14\sum_q r_{c,q}^2-3.5.
$$

The measured chroma coordinate $a_c$ is modeled as

$$
a_c=\beta_0+\beta_1z_c+
\sum_{d\in\{U,D,L,R\}}\beta_dz_d+
\beta_m m_c+\beta_h h_c+\epsilon_c.
$$

The luminance moments remain fixed during chroma refinement. Neighbor signs come
from the previous pass. Up to five passes run per axis, stopping when all signs
are unchanged. Supports use even cell coordinates inside an inclusive radius-24
square around nodes spaced 16 cells apart. A supported cell and its four neighbors
must belong to the data mask. Boundary rows use wrapped neighbors to construct
the basis, but are excluded from fitting by the interior validity mask.

At least eight rows are required. Fit unweighted least squares, compute median
residual and MAD, set $s=\max(0.5,1.4826\operatorname{MAD})$, then solve once more
with weights $\min(1,2.5s/\max(|e_c|,10^{-6}))$. Weighted residual variance is
$\sum_c w_ce_c^2/\sum_c w_c$. The normalized normal-equation pivot cutoff is
$10^{-12}$. Coefficients and variance are bilinearly interpolated; interpolation
fractions are rounded to multiples of $1/32$. Every nonzero-weight corner must
have a model.

Subtract all terms except $\beta_1z_c$ to obtain $t_c$. The output sign is positive
exactly when $t_c>0$. This assumes a positive self response; the fit does not
separately enforce $\beta_1>0$. For $d_+=(t_c-\beta_1)^2$ and
$d_-=(t_c+\beta_1)^2$, confidence is

$$
C_c=\frac{|d_+-d_-|}{d_++d_-+2\sigma^2}.
$$

An unavailable interpolated chroma model sets that bit's confidence to zero and
leaves its decision unchanged. An axis with no fitted node returns an error.

## Coupled luminance model

Expand the cell grid of side $G$ into a quadrant grid of side $S=2G$. Quadrant
$(c,q)$ lies at

$$
x=2(c\bmod G)+(q\bmod2),\qquad
y=2\lfloor c/G\rfloor+\lfloor q/2\rfloor.
$$

Let $v_p$ be the measured `mixed` luminance at quadrant position $p$, $r_p$ its
current rank, and $r_{p+d}$ a neighbor rank. Directions are ordered **right,
left, down, up**, including neighbors in adjacent cells. For a local node, fit

$$
v_p=\alpha_{r_p}+
\sum_{d\in\{R,L,D,U\}}\sum_{k=1}^{3}
\lambda_{d,k}\,\mathbf1[r_{p+d}=k]+
\gamma_b z_{b,c(p)}+\gamma_r z_{r,c(p)}+\epsilon_p.
$$

The 18 coefficients are stored in this exact order:

| Indices | Meaning |
| --- | --- |
| 0–3 | Self levels $\alpha_0,\alpha_1,\alpha_2,\alpha_3$ |
| 4–6 | Right-neighbor contributions for ranks 1, 2, 3 |
| 7–9 | Left-neighbor contributions for ranks 1, 2, 3 |
| 10–12 | Down-neighbor contributions for ranks 1, 2, 3 |
| 13–15 | Up-neighbor contributions for ranks 1, 2, 3 |
| 16–17 | Shared chroma contributions $\gamma_b,\gamma_r$ |

Rank zero is the reference category for each neighbor: $\lambda_{d,0}=0$.
The four self-level indicators already sum to one, so there is no separate
intercept. The three neighbor contributions are independent fitted values; no
linear relation in rank is imposed. The model assumes additive effects, ordered
self levels, adequate rank diversity, and a channel that changes slowly across
local supports. It includes neither diagonal neighbors nor pairwise interactions.

### Packed samples and supports

A sample stores its self rank, measured value, and up to six active coefficient
indices: at most four nonzero neighbor ranks, followed by both chroma terms. A
bit mask identifies negative chroma signs by active slot. Positive neighbor
indicators have no negative bit. This is a sparse representation of the equation
above, not another parameterization. Accumulation follows sample order, direction
order, then chroma order; retaining that order preserves floating-point results.

Only interior quadrants whose own measurement and four neighbor measurements are
finite and belong to masked data cells are eligible. Fit samples additionally
require even cell coordinates in both axes:
$\lfloor x/2\rfloor\bmod2=\lfloor y/2\rfloor\bmod2=0$.
All eligible quadrants of such a cell are retained.

Node spacing and support radius, in quadrant coordinates, are

$$
\Delta=\max\left(16,\left\lceil\frac{S-1}{32}\right\rceil\right),
\qquad R=\left\lfloor\frac{3\Delta}{2}\right\rfloor.
$$

Each coordinate list starts at zero, advances by $\Delta$ while less than $S-1$,
and appends $S-1$. A node uses sampled quadrants strictly inside its radius-$R$
disk, clipped to the image. Supports and sample ordering are fixed across passes;
active coefficients change with the preceding pass's ranks.

### Fit and acceptance

Each self rank must have at least eight samples in a node's support. Let $X$ be
the sparse design matrix and $\theta$ the 18 coefficients. First solve

$$
(X^\mathsf T X)\theta^{(0)}=X^\mathsf T v.
$$

For residuals $e_p^{(0)}=v_p-X_p\theta^{(0)}$, calculate

$$
\mu=\operatorname{median}_p e_p^{(0)},\qquad
s=\max\left(1,1.4826\operatorname{median}_p|e_p^{(0)}-\mu|\right),
\qquad
w_p=\min\left(1,\frac{2.5s}{\max(|e_p^{(0)}|,10^{-6})}\right).
$$

The weights use the **uncentered** residual magnitude; centering is used only for
the MAD scale. Perform exactly one weighted solve:

$$
(X^\mathsf T W X)\widehat\theta=X^\mathsf T Wv,
\qquad
\sigma^2=\frac{\sum_p w_p(v_p-X_p\widehat\theta)^2}{\sum_p w_p}.
$$

There is no ridge term or iterative reweighting within one fit. Both solves
normalize the normal system by $D_{jj}=\sqrt{(X^\mathsf T WX)_{jj}}$, solve
$D^{-1}(X^\mathsf T WX)D^{-1}$ using Gauss–Jordan elimination with maximum absolute
partial pivots, and divide the solved coefficients by $D_{jj}$. Nonfinite or
$D_{jj}\le\texttt{f64::EPSILON}$ rejects the node. Any normalized pivot magnitude
below $10^{-4}$, or nonfinite final coefficient, also rejects it.

Define the minimum self-level gap

$$
\delta=\min_{k=0,1,2}(\widehat\alpha_{k+1}-\widehat\alpha_k).
$$

Accept only $\delta\ge8$, finite $\sigma^2$, and
$\sqrt{\sigma^2}/\delta<0.4$. These are rejection gates, not constraints in the
least-squares optimization. Negative or too-close level ordering is rejected
rather than repaired.

### Interpolation, inference, and confidence

Between four neighboring field nodes, use the bilinear weights
$((1-u)(1-v),u(1-v),(1-u)v,uv)$ to interpolate each coefficient and the residual
variance. Weights at or below `f64::EPSILON` are skipped. Every remaining corner
must have a model; missing corners are not renormalized. The final interval may
be shorter than the nominal spacing. Fractions are not quantized here.

With interpolated parameters at $p$, subtract the neighbor and chroma contributions
and compare the resulting signal against each self level:

$$
t_p=v_p-\sum_d\lambda_{d,r_{p+d}}
-\gamma_bz_{b,c(p)}-\gamma_rz_{r,c(p)},\qquad
d_{p,k}=(t_p-\alpha_k)^2,\qquad
\widehat r_p=\operatorname*{arg\,min}_{k\in\{0,1,2,3\}}d_{p,k}.
$$

Equal distances choose the first rank under the implementation's total ordering.
Convert the selected rank back to Gray bits
$g(k)=k\mathbin{\oplus}(k\mathbin{\gg}1)$. For bit position $b\in\{0,1\}$,

$$
D_{p,b,a}=\min_{k:\,((g(k)\gg b)\,\&\,1)=a}d_{p,k},
\qquad
C_{p,b}=\frac{|D_{p,b,0}-D_{p,b,1}|}
{D_{p,b,0}+D_{p,b,1}+2\max(\sigma^2,1)}.
$$

Confidence is a normalized separation score, not a calibrated probability. Fit,
interpolation, distances, and variance use `f64`; returned confidence uses `f32`.

Each pass fits with the previous ranks and writes all next ranks simultaneously.
Run at most five passes and stop when no rank changes. A masked-out cell preserves
its complete input decision. A quadrant without eligible data or an interpolated
model restores its input Gray bits, confidence, and rank, even if an earlier pass
changed it. Chroma bits and their confidence are preserved throughout this stage.

## Certified evidence

`initialize_from_certificates` takes the same alphabet seed and delegates fitting
to `recovery_fit`. In `decision_from_evidence`, supported bits use the sign of the
fitted value and confidence $\tanh(|v_b|/2)$; unsupported bits retain the seed.
Certified bits override both sources with their specified value and confidence
one. Zero supported evidence yields a zero bit and zero confidence. This route
does not invoke the blind binary chroma or coupled luminance stages described above.
