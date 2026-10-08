# CW Luma 10 format

CW Luma 10 encodes binary data in a square RGB image. Each data cell has four independently selected luminance levels and two shared chroma signs, carrying ten coded bits. Finders, calibration pilots, and frame metadata occupy reserved cells inside the same grid.

There is one optical layout, with Standard and Low error-correction profiles. CWT is the supported envelope for transferring a file across multiple images. The format and APIs are experimental; the [compatibility corpus](tests/fixtures/README.md) records the current packet bytes, repair symbols, and raster pixels.

This document specifies the raster and byte encodings, then explains how the current decoder recovers them. Sampling, calibration, and retry strategies may improve without changing the format.

Equations use LaTeX with inline math and fenced `math` blocks. Other code blocks retain exact bit operations and byte-order algorithms. Mathematical coordinates are in cells unless explicitly labeled as pixels.

## 1. From file bytes to pixels

A single image carries one optical packet:

```text
application bytes
  → optional lossless compression
  → packet header + stored payload + zero padding
  → systematic Reed–Solomon blocks
  → byte interleaving
  → ten-bit cell symbols
  → quadrant luminance + shared chroma
  → RGB raster with finders, metadata, and pilots
```

A transfer adds a layer before optical encoding:

```text
file → fixed-size source chunks → groups of sources + four repair chunks
     → one CWT envelope per chunk → one optical packet per image
```

These layers have different units:

| Term | Meaning |
| --- | --- |
| Cell | A square region of the image containing four quadrants |
| Cell symbol | A ten-bit value, `0..1023`, assigned to one data or pilot cell |
| Optical codeword | A 255-byte Reed–Solomon block within one image |
| Optical packet | A 12-byte header, stored payload, and canonical zero padding |
| Transfer symbol / shard | One source or repair chunk, carried in a CWT envelope |
| Transfer group | Up to 20 or 60 source shards, followed by four repair shards |

Optical ECC repairs erroneous bytes inside an image. Transfer repair reconstructs missing chunks after whole images have failed to decode or were never captured. The two codes are independent.

## 2. Optical profiles

| Profile | Frame identifier | Raw packet magic | Compressed packet magic | RS data bytes `D` | RS parity bytes `P` |
| --- | --- | --- | --- | ---: | ---: |
| Standard | `0xCD` | `CWI4` | `CWI5` | 159 | 96 |
| Low | `0xCE` | `CWE4` | `CWE5` | 207 | 48 |

Both profiles use the same cell alphabet, pilots, metadata placement, and data-cell traversal. The frame identifier tells the scanner which ECC parameters and packet magics to expect. A packet magic from the other profile is invalid.

The CLI defaults to `--ecc standard` and uncompressed payloads. `--ecc low` increases capacity at the cost of error-correction margin. `--compression auto` enables the selection rule in section 6.

## 3. Cell modulation

### Ten bits in six measured coordinates

A cell of even pixel pitch `s` is divided into four `s/2 × s/2` quadrants. Bit numbers below refer to the integer cell symbol; bit 0 is its least significant bit.

```text
                 shared Cb: bit 8
                 shared Cr: bit 9
              ┌───────────┬───────────┐
              │ TL, q = 0 │ TR, q = 1 │
              │ bits 1:0  │ bits 3:2  │
              ├───────────┼───────────┤
              │ BL, q = 2 │ BR, q = 3 │
              │ bits 5:4  │ bits 7:6  │
              └───────────┴───────────┘
```

Each quadrant selects one of four luminance levels using a two-bit Gray code:

```text
gray = (symbol >> (2*q)) & 3
rank = gray ^ (gray >> 1)
```

For quadrant rank $`r_q`$, normalized luminance $`t_q`$ and emitted luminance $`Y_q`$ are:

```math
t_q = 2r_q-3,\qquad Y_q = 128+28t_q.
```

| Gray bits | Binary rank | Normalized luminance | `Y` |
| --- | ---: | ---: | ---: |
| `00` | 0 | −3 | 44 |
| `01` | 1 | −1 | 100 |
| `11` | 2 | +1 | 156 |
| `10` | 3 | +3 | 212 |

Adjacent luminance levels differ by one coded bit. Both chroma components are constant across the cell: bit 8 selects `Cb = −20` for zero and `+20` for one; bit 9 does the same for `Cr`.

Every pixel in a quadrant receives the same RGB value:

```math
\begin{aligned}
R &= Y + 1.402C_r,\\
G &= Y - \frac{0.299(1.402)C_r+0.114(1.772)C_b}{0.587},\\
B &= Y + 1.772C_b.
\end{aligned}
```

Channels are rounded to the nearest integer. This calculation operates directly on 8-bit RGB code values; it does not linearize them through an sRGB transfer function. All 1,024 symbols lie within RGB channel values 9 through 247, so the alphabet needs no clipping.

The alphabet size and coded bits per cell are:

```math
|\mathcal{S}|=4^4\,2^2=1024,\qquad \log_2|\mathcal{S}|=10.
```

The ten bits occupy six measured coordinates: four quadrant luminances, mean Cb, and mean Cr. They do not require ten independent camera channels.

### Relation to directional contrast and transforms

The four luminances can equivalently be expressed in a 2×2 Haar basis. For $`a=Y_{\mathrm{TL}}`$, $`b=Y_{\mathrm{TR}}`$, $`c=Y_{\mathrm{BL}}`$, and $`d=Y_{\mathrm{BR}}`$, one normalization is:

```math
\begin{bmatrix}
\mu\\ h\\ v\\ d_{\mathrm{xy}}
\end{bmatrix}
=\frac14
\begin{bmatrix}
1&1&1&1\\
1&-1&1&-1\\
1&1&-1&-1\\
1&-1&-1&1
\end{bmatrix}
\begin{bmatrix}a\\b\\c\\d\end{bmatrix}.
```

Here $`\mu`$ is the mean, $`h`$ is left-right contrast, $`v`$ is top-bottom contrast, and $`d_{\mathrm{xy}}`$ is checkerboard contrast. If $`K`$ denotes the 4×4 matrix of $`\pm1`$ entries above, then $`KK^\mathsf{T}=4I`$, so the inverse is $`[a,b,c,d]^\mathsf{T}=K^\mathsf{T}[\mu,h,v,d_{\mathrm{xy}}]^\mathsf{T}`$.

This is a change of coordinates for the same four values. The allowed contrast coefficients are coupled by the four-level quadrant alphabet; they are not additional independent data layers. The current classifier operates on calibrated quadrant values. It does not perform a full-image DFT or wavelet decomposition to recover the payload. Local frequency/phase estimation is used for camera-grid registration, described in section 9.

## 4. Grid layout and capacity

Let `g` be the number of cells along the encoded grid and `s` the source pixel pitch. Grid coordinates are zero-based, with x increasing rightward and y downward. Intervals in this section are half-open.

```math
n_{\mathrm{px}}=(g+4)s,\qquad
40\le g\le510,\qquad
s\in\{2,4,\ldots,64\},\qquad
n_{\mathrm{px}}\le8192.
```

The extra four cells are a two-cell white quiet zone on every side. Grid cell `(x,y)` begins at raster pixel `((x+2)*s, (y+2)*s)`. Finders and metadata are integrated into the grid; they do not add another external frame.

| Region | Grid coordinates | Purpose |
| --- | --- | --- |
| Four corner reservations | Each combination of x and y in `[0,8)` or `[g−8,g)` | Finders and inward white separators |
| Four pilot patches | Origins `(0,8)`, `(g−8,8)`, `(0,g−12)`, `(g−8,g−12)`; each 8×4 cells | Known calibration symbols |
| Top metadata reservation | x `[8,g−8)`, y `[1,6)` | White guards around the metadata stripe |
| Bottom metadata reservation | x `[8,g−8)`, y `[g−6,g−1)` | Repeated metadata and guards |
| Data cells | Every other cell, row-major | Interleaved coded bits, then zero fill |

Each finder is a black 7×7 outer ring with a black 3×3 center and a white ring between them. It sits flush with the two outer edges of its 8×8 corner reservation. The four finder centers are 5.5 cells from their nearest raster edges. Pilots sit immediately inward of the top and bottom finders.

The five-row metadata reservations leave data-bearing strips above and below them. Those colored strips are useful data cells. The four pilot patches, by contrast, are deliberately identical between images of the same layout; they provide stable calibration references during transfers.

### Capacity calculation

The reservations are disjoint and consume $`10g+224`$ cells. The data-cell count $`A`$, optical codeword count $`B`$, and stored payload capacity $`C`$ are:

```math
A(g)=g^2-10g-224,\qquad
B(g)=\left\lfloor\frac{10A(g)}{8\cdot255}\right\rfloor,\qquad
C(g)=D\,B(g)-12.
```

For a stored payload length $`L`$, the encoder selects the smallest supported grid satisfying $`D\,B(g)\ge L+12`$. It uses every complete codeword available in that grid and zero-pads the packet to $`D\,B(g)`$ bytes. Any remaining cell bits are zero fill. The image-side bound can reduce the maximum grid at larger pitches.

For a raw 2,048-byte payload:

| Profile | Grid `g` | Codewords `B` | Capacity | Raster at `s=2` | Raster at `s=8` |
| --- | ---: | ---: | ---: | --- | --- |
| Standard | 59 | 13 | 2,055 bytes | 126×126 | 504×504 |
| Low | 53 | 10 | 2,058 bytes | 114×114 | 456×456 |

For example, the Low profile at $`g=53`$ gives:

```math
A=53^2-10(53)-224=2055,\qquad
B=\left\lfloor\frac{20550}{2040}\right\rfloor=10,\qquad
C=207(10)-12=2058.
```

At `g=510`, capacity is 198,420 stored bytes for Standard or 258,324 for Low. A two-pixel pitch gives one raster pixel per quadrant; it is a digital minimum, not a demonstrated camera-scanning size. Changing pitch scales the image without changing the payload alphabet or ECC.

## 5. Optical packet and interleaving

All multibyte integer fields are unsigned little-endian. The packet before Reed–Solomon encoding is:

| Offset | Bytes | Field |
| ---: | ---: | --- |
| 0 | 4 | ASCII packet magic from section 2 |
| 4 | 4 | Stored payload length `L` (`u32`) |
| 8 | 4 | CRC-32 (`u32`) |
| 12 | `L` | Raw application bytes or compressed envelope |
| `12+L` | `D*B−12−L` | Zero padding |

CRC-32/ISO-HDLC covers `packet[0..8]` followed by the stored payload. It excludes the checksum field itself and the padding. The reflected polynomial is `0xEDB88320`, with initial value and final XOR `0xFFFFFFFF`; the check value for `123456789` is `0xCBF43926`.

The length must fit both the decoded packet and the capacity implied by the grid. Every padding byte must be zero. Empty raw payloads are valid. These checks apply whether the packet was accepted directly or recovered through ECC.

### Optical Reed–Solomon code

Split the padded packet into `B` consecutive blocks of `D` bytes. Each becomes a systematic 255-byte codeword: data first, followed by `P=255−D` parity bytes.

Arithmetic is over $`\mathrm{GF}(256)`$, using primitive polynomial `0x11D` and primitive element $`\alpha=2`$. Bytes are polynomial coefficients in descending degree order. For $`P`$ parity bytes, the generator is:

```math
G_P(x)=\prod_{j=0}^{P-1}(x+\alpha^j).
```

For data polynomial $`m(x)`$, append the remainder $`r(x)`$ to form the codeword:

```math
r(x)=m(x)x^P\bmod G_P(x),\qquad
c(x)=m(x)x^P+r(x).
```

Addition and subtraction in this field are XOR. This fixes the parity convention used by the vendored encoder.

Standard corrects up to 48 unknown erroneous bytes per codeword; Low corrects up to 24. With $`e`$ unknown byte errors and $`u`$ known erasure positions, the decoding bound is:

```math
2e+u\le P.
```

Errors here are byte errors within an individual codeword, not percentages of the image area.

### Byte and bit order

Let `C[b][j]` be byte `j` of codeword `b`. Transmit bytes in this order:

```text
C[0][0], C[1][0], …, C[B−1][0],
C[0][1], C[1][1], …, C[B−1][1],
…
C[0][254], …, C[B−1][254]
```

Equivalently, for codeword index $`b`$ and byte position $`j`$:

```math
\mathrm{stream}[jB+b]=C[b][j].
```

This distributes adjacent stream damage across codewords.

Read each stream byte most significant bit first. Consecutive groups of ten bits form cell symbols, also most significant bit first: the first stream bit becomes symbol bit 9, and the tenth becomes bit 0. Map those integers using section 3 and visit data cells in row-major order, skipping reservations. Each 255-byte codeword contributes 2,040 bits, so the coded stream fills exactly `204*B` cells with no partial final symbol. All remaining data cells contain symbol zero.

## 6. Optional compression

Automatic compression uses Zstandard level 3. The compressed envelope is:

| Offset | Bytes | Field |
| ---: | ---: | --- |
| 0 | 4 | Original uncompressed length (`u32`, little-endian) |
| 4 | variable | Exactly one standard Zstandard frame |

Let $`L_c`$ include the four-byte compressed-envelope header and $`L_o`$ be the original length. The encoder chooses compression only if it saves at least one optical RS data block:

```math
\left\lceil\frac{L_c+12}{D}\right\rceil
<
\left\lceil\frac{L_o+12}{D}\right\rceil.
```

Otherwise it emits the original bytes with the raw packet magic. Grid selection uses the stored length; minimum-grid reservations still apply, so saving a block does not always shrink the raster.

The compressed packet magic selects decompression. Frame metadata, modulation, pilots, and ECC are unchanged. The Zstandard content-size field must be present and equal to the envelope's original length. The decoder limits output to 16 MiB and the window to `2^24` bytes, checks limits before output allocation, and requires exactly the declared output size. Skippable frames, concatenated frames, trailing input, truncated frames, and length mismatches are rejected. Optical CRC and canonical packet padding are validated before decompression.

In reported layouts, `payload_bytes` counts original bytes; `encoded_payload_bytes` counts stored bytes, including the four-byte compressed-envelope length when used. `payload_capacity_bytes` always refers to stored bytes. `compression` is `none` or `zstd`. `net_bits_per_pixel` uses original payload bits divided by nominal raster area, so compression can raise that metric without changing modulation density.

## 7. Metadata stripes

Top and bottom stripes carry identical extended Hamming(32,26) words. The 26 information bits are:

```text
check = CRC32([profile_id, (g >> 8) as u8, g as u8]) & 0x7F
data  = (profile_id << 18) | (g << 7) | check
```

The checksum input is three bytes, with the grid's high byte before its low byte. `profile_id` occupies eight bits, `g` eleven bits, and `check` seven bits. The grid must still satisfy section 4's bounds.

To form the word, number positions 1 through 32; position `p` is integer bit `p−1`. Fill positions 1 through 31 that are not powers of two with successive information bits, least significant bit first. Positions 1, 2, 4, 8, and 16 are even parity over positions whose index contains the corresponding bit. Position 32 supplies overall even parity. This corrects a single bit error and detects double bit errors; it does not guarantee detection of larger errors.

Render the completed word most significant bit first from left to right, black for one and white for zero. Stripe height is three cells. In grid coordinates, the top stripe covers y `[2,5)` and the bottom y `[g−5,g−2)`.

Horizontal placement is measured between finder centers. In raster coordinates, let `c=5.5*s` and `span=(g−7)*s`. The stripe runs from `c+0.15*span` to `c+0.85*span`, divided into 32 equal bit widths. A pixel belongs to a bit according to its center `x+0.5`; bit boundaries need not align with cell boundaries. Both copies have the same left-to-right order.

## 8. Walsh–Hadamard calibration pilots

### Construction and bit mapping

Each 8×4 pilot patch contains the same 32 known symbols in row-major order. Its bit sequences are selected columns of a 32×32 Sylvester Hadamard matrix in natural binary order. For row index $`i`$ and column mask $`m`$, define:

```math
H_{i,m}=(-1)^{\mathrm{popcount}(i\mathbin{\&}m)},
\qquad i,m\in\{0,\ldots,31\}.
```

The bitwise AND computes the overlap of the five-bit index and mask. Popcount parity supplies their binary inner product. The encoder uses these ten nonzero, distinct column masks in this exact order:

```math
(m_0,\ldots,m_9)=(1,2,4,8,16,3,5,6,9,10).
```

Map each selected column to a binary bit $`b_j`$ and a signed coordinate $`z_j`$:

```math
b_j(i)=\frac{1-H_{i,m_j}}{2},
\qquad z_j(i)=2b_j(i)-1=-H_{i,m_j}.
```

For quadrant $`q\in\{0,1,2,3\}`$, combine two columns into a binary rank, then convert that rank to the Gray field used by section 3:

```math
r_q(i)=2b_{2q}(i)+b_{2q+1}(i),\qquad
t_q(i)=2r_q(i)-3=2z_{2q}(i)+z_{2q+1}(i).
```

The exact integer assembly is:

```text
rank[q] = 2*b[2*q] + b[2*q+1]
gray[q] = rank[q] ^ (rank[q] >> 1)
symbol  = sum(gray[q] << (2*q), q=0..3) | (b[8] << 8) | (b[9] << 9)
```

The final two latent coordinates are $`t_4=z_8`$ and $`t_5=z_9`$. They select $`C_b=20t_4`$ and $`C_r=20t_5`$. This sequence is part of the raster format; changing the masks or their order changes the pilots a decoder must expect.

### Why these pilots separate channel mixing

Hadamard columns obey:

```math
\sum_{i=0}^{31}H_{i,m}H_{i,n}
=\begin{cases}32,&m=n,\\0,&m\ne n.\end{cases}
```

For distinct masks, the product is the column indexed by $`m\oplus n`$, which is nonzero and balanced. Every selected column is also orthogonal to the constant column $`m=0`$, giving zero mean.

Each luminance coordinate combines a disjoint pair of signed columns with weights 2 and 1. Its mean square is therefore $`2^2+1^2=5`$; each chroma coordinate has mean square 1. For the six-vector $`\mathbf t_i`$, the exact pilot moments are:

```math
\sum_{i=0}^{31}\mathbf t_i=\mathbf0,\qquad
\Gamma=\sum_{i=0}^{31}\mathbf t_i\mathbf t_i^\mathsf{T}
=\mathrm{diag}(160,160,160,160,32,32).
```

Thus all six latent coordinates are orthogonal over the complete pilot patch, even though a luminance coordinate itself has four levels. The construction supplies an explicit, diagonal normalization for estimating a channel model.

### Affine calibration from measured pilots

For each pilot, the decoder measures a feature vector containing four quadrant luminances and the two chroma components averaged across the quadrants:

```math
\mathbf y_i=
[Y_{\mathrm{TL}},Y_{\mathrm{TR}},Y_{\mathrm{BL}},Y_{\mathrm{BR}},
\overline C_b,\overline C_r]^\mathsf{T}.
```

Within one patch, fit the affine model

```math
\mathbf y_i=\mathbf c+A\mathbf t_i+\boldsymbol\epsilon_i,
```

where $`\mathbf c`$ is the mean offset, $`A`$ is a 6×6 mixing matrix, and $`\boldsymbol\epsilon_i`$ is residual error. The pilot moments give the least-squares estimates directly:

```math
\widehat{\mathbf c}=\frac1{32}\sum_i\mathbf y_i,\qquad
\widehat A=
\left(\sum_i(\mathbf y_i-\widehat{\mathbf c})\mathbf t_i^\mathsf{T}\right)\Gamma^{-1}
=\left(\sum_i\mathbf y_i\mathbf t_i^\mathsf{T}\right)\Gamma^{-1}.
```

The second equality follows from the pilots' zero mean. Each of the first four columns divides its cross-moment by 160; the last two divide by 32. The implementation computes these selected correlations directly; it does not run a full fast Walsh–Hadamard transform.

The decoder inverts $`\widehat A`$, evaluates residuals in latent coordinates, and rejects degenerate, ill-conditioned, or poorly fitting patches. Four repeated patches allow the fit to vary with image position. For valid-patch weights $`w_p(\mathbf x)`$, the implementation interpolates the center and inverse matrix separately:

```math
\begin{aligned}
\mathbf c(\mathbf x)&=\sum_p w_p(\mathbf x)\widehat{\mathbf c}_p,\\
B(\mathbf x)&=\sum_p w_p(\mathbf x)\widehat A_p^{-1},\\
\widehat{\mathbf t}(\mathbf x)&=
B(\mathbf x)\bigl(\mathbf y(\mathbf x)-\mathbf c(\mathbf x)\bigr).
\end{aligned}
```

The weights are bilinear between pilot-patch centers, clamped outside their span and renormalized over usable patches. If that leaves zero weight, usable patches receive equal weights. The matrix being interpolated is the inverse; the implementation does not interpolate forward matrices and then invert the result.

### Worked pilot and calibration calculation

For pilot index $`i=5`$, the mask parities are:

```math
(b_0,\ldots,b_9)=(1,0,1,0,0,1,0,1,1,0).
```

This gives ranks $`(2,2,1,1)`$, Gray fields $`(3,3,1,1)`$, and:

```math
\begin{aligned}
\mathrm{symbol}
&=3+3(2^2)+1(2^4)+1(2^6)+1(2^8)=351,\\
\mathbf t_5&=(1,1,-1,-1,1,-1)^\mathsf{T},\\
(Y_{\mathrm{TL}},Y_{\mathrm{TR}},Y_{\mathrm{BL}},Y_{\mathrm{BR}})
&=(156,156,100,100),\\
C_b&=20,\qquad C_r=-20.
\end{aligned}
```

The emitted symbol integer is `0x15F`. After RGB rounding, each upper quadrant is `(128,163,191)` and each lower quadrant is `(72,107,135)`.

To illustrate channel estimation separately from that rounding, suppose a hypothetical affine channel has offset $`\mathbf c=(128,128,128,128,0,0)^\mathsf{T}`$, diagonal gains $`(28,28,28,28,20,20)`$, and one additional coupling $`A_{0,1}=7`$. Then the first measured coordinate is $`y_{i,0}=128+28t_{i,0}+7t_{i,1}`$. Across all 32 pilots:

```math
\sum_i y_{i,0}t_{i,0}=28(160)=4480,\qquad
\sum_i y_{i,0}t_{i,1}=7(160)=1120.
```

Dividing by 160 recovers the gain 28 and coupling 7. Pilot 5 is observed as $`(163,156,100,100,20,-20)^\mathsf{T}`$ in this illustrative channel. The top-left 2×2 inverse is:

```math
\begin{bmatrix}28&7\\0&28\end{bmatrix}^{-1}
=
\begin{bmatrix}1/28&-1/112\\0&1/28\end{bmatrix}.
```

Applying it recovers the first coordinate:

```math
\widehat t_0=\frac{163-128}{28}-\frac{156-128}{112}
=1.25-0.25=1.
```

The remaining coordinates recover as $`(1,-1,-1,1,-1)`$, yielding the original ten-bit symbol after quantization.

## 9. Camera decoding

The wire format specifies the transmitted pixels and validation rules. The following describes the current implementation, not additional packet fields or a required decoder architecture.

1. **Find the grid.** Detect four finder markers, refine their centers, read metadata, and try rotations and reflections. Pilot agreement helps select orientation. A projective transform maps grid coordinates into the original camera image.
2. **Register and sample.** Sample the quadrant lattice through that transform. When registration is needed, local windowed sinusoidal projections of edge magnitude estimate the grid phase at two cycles per cell along x and y. Phase unwrapping produces a displacement mesh. RGB and luminance meshes can be estimated separately to compensate for relative color-channel displacement.
3. **Equalize and calibrate.** Known pilots fit spatial mixing from neighboring samples. Regularized equalization, the six-coordinate affine calibration, and locally estimated luminance levels compensate for blur, color shifts, and spatial variation within their model limits.
4. **Classify cells.** Quantize four luminance coordinates to Gray ranks and two chroma coordinates to signs. Keep bit confidence as well as hard decisions. A neighbor-conditioned luminance model can refine decisions after a failed decode.
5. **Recover the packet.** Undo bit packing and byte interleaving. Try the systematic data first; if profile magic, length, CRC, padding, and any decompression validate, return immediately. Otherwise use confidence-derived byte erasures with Reed–Solomon correction and apply the same packet checks again.
6. **Deliver bytes.** Static decoding returns the application bytes. Transfer mode parses those bytes as a CWT envelope and passes validated source/repair shards to the transfer receiver.

### Local Fourier phase alignment

Pilot calibration estimates what a measured value means. Grid registration estimates where that value should be sampled. The registration code uses a single-frequency complex Fourier projection of edge magnitude along each axis. It does not use the Walsh–Hadamard pilot transform.

Quadrant boundaries form a lattice with spacing $`\tau=1/2`$ cell. Its spatial frequency is $`f_0=1/\tau=2`$ cycles per cell and angular frequency is $`\omega_0=2\pi f_0=4\pi`$ radians per cell. Individual boundaries may have little contrast, so the estimator accumulates evidence over a local window.

For a mesh node $`\mathbf q`$, sample each channel $`s_c`$ on a 51×51 lattice:

```math
\mathbf p_{mn}=\mathbf q+\frac18(m,n),\qquad m,n=-25,\ldots,25.
```

The interior 49×49 samples provide central differences, with step $`h=1/8`$ cell and Gaussian weights:

```math
\begin{aligned}
D_{c,a}(\mathbf p)
&=\left|s_c(\mathbf p+h\mathbf e_a)-s_c(\mathbf p-h\mathbf e_a)\right|,\\
w_{mn}
&=\exp\left(-\frac{m^2+n^2}{2\cdot12^2}\right)
=\exp\left(-\frac{\|\mathbf p_{mn}-\mathbf q\|^2}{2(1.5)^2}\right).
\end{aligned}
```

Here $`a`$ is x or y, $`\mathbf e_a`$ is its unit vector, and the Gaussian standard deviation is 1.5 cells. The implementation uses the absolute difference without dividing by $`2h`$. A sample contributes only if its center and both neighbors are valid: inside the grid, outside masked finder/metadata regions, and successfully sampled from the image.

For that valid set $`\mathcal V_a`$, accumulate the complex response and total edge magnitude:

```math
Z_{c,a}=\sum_{(m,n)\in\mathcal V_a}
w_{mn}D_{c,a}(\mathbf p_{mn})e^{-\mathrm{i}4\pi p_{mn,a}},
\qquad
T_{c,a}=\sum_{(m,n)\in\mathcal V_a}w_{mn}D_{c,a}(\mathbf p_{mn}).
```

The exponential uses the **absolute grid coordinate** $`p_{mn,a}`$, not the offset from the window center. Its real and imaginary parts are accumulated using cosine and negative sine. Let $`W_a=\sum_{\mathcal V_a}w_{mn}`$ and $`W_{\mathrm{full}}`$ be the weight sum over all 49×49 interior positions, before validity masking. The current implementation estimates phase only when $`T_{c,a}>1`$ and $`W_a\ge0.3W_{\mathrm{full}}`$:

```math
\delta_{c,a}=
-\frac{\mathrm{atan2}(\mathrm{Im}\,Z_{c,a},\mathrm{Re}\,Z_{c,a})}{4\pi},
\qquad
\kappa_{c,a}=\frac{|Z_{c,a}|}{T_{c,a}}.
```

Displacement $`\delta`$ is in cells, with ambiguity modulo half a cell. Coherence $`\kappa`$ lies between zero and one by the triangle inequality: edges aligned to one lattice phase reinforce each other, while inconsistent phases cancel.

For an ideal edge pattern shifted by $`+0.1`$ cell, the response has the phase:

```math
Z=Ae^{-\mathrm{i}4\pi(0.1)}
=A(0.309017-0.951057\mathrm{i}),\qquad A>0.
```

The angle is approximately $`-1.256637`$ radians, so:

```math
\delta=-\frac{-1.256637}{4\pi}\approx+0.1\ \text{cell}.
```

At a local observed pitch of eight camera pixels per cell, that corresponds to approximately 0.8 camera pixel. This is an ideal phase calculation; finite windows, masked regions, blur, and the particular symbol pattern can bias the measured response.

### Unwrapping and channel-specific sampling

The displacement mesh has 17 nodes per side for $`g\le200`$ and 33 for larger grids, spanning cell-center coordinates from $`0.5`$ to $`g-0.5`$. The decoder requires sufficient confident nodes on both axes. For each axis, it starts from a confident node near the top-left pilot region and traverses neighbors in confidence order.

If node $`n`$ has principal displacement $`\delta_n`$ and an already visited parent with displacement $`d_{\mathrm{parent}}`$, its continuous branch is chosen as:

```math
d_n=\delta_n+\frac12\,\mathrm{round}
\left(\frac{d_{\mathrm{parent}}-\delta_n}{1/2}\right).
```

For example, $`\delta_n=-0.24`$ beside $`d_{\mathrm{parent}}=+0.24`$ unwraps to $`+0.26`$ cell, preserving a 0.02-cell local change instead of introducing a half-cell jump. Nodes with $`\kappa\le0.15`$ first inherit their parent's displacement, then are filled by 64 neighbor-average sweeps. Confident nodes retain their measurements; integrated finder-corner nodes remain fixed at zero.

Bilinear interpolation yields a displacement field $`\mathbf d_c(\mathbf p)`$. The corresponding sampling point is $`\mathbf p_c=\mathbf p+\mathbf d_c(\mathbf p)`$, which is then projected into the original camera image. RGB and luminance $`Y=0.299R+0.587G+0.114B`$ can have independently estimated fields. Available RGB positions are aligned to the half-cell-equivalent position nearest the luminance position:

```math
\mathbf p_c\leftarrow
\mathbf p_c-\frac12\,\mathrm{round}
\left(\frac{\mathbf p_c-\mathbf p_Y}{1/2}\right),
\qquad c\in\{R,G,B\},
```

with rounding applied componentwise. Missing color fields use the luminance position; a missing luminance field uses the unadjusted grid position. This permits relative channel registration while resolving the periodic branch consistently. It does not add payload dimensions or guarantee recovery from severe chromatic aberration or aliasing. These window sizes, confidence gates, and interpolation choices describe the current decoder and are not wire-format constants.

### Packet validation and observation recovery

The direct systematic-data path does not check or repair parity bytes. `corrected_bytes` is zero on that path; otherwise it counts locations reported by RS, including erased positions that may already have held the right value. `erased_bytes` counts low-confidence bytes across data and parity even when the direct path succeeds. Confidence thresholds are implementation details.

For a static code, `PhotoRecovery` can combine at most four accepted observations, retaining decoder-certified RS blocks and fitted signal evidence. The decision-directed fits, coefficient layout, acceptance gates, and confidence calculations are detailed in the [recovery signal model](docs/recovery-model.md). Layout, alignment, correlation, duplicate, and contradiction checks constrain fusion; final packet validation is still required. `TransferScanner` reuses geometry across changing images but does not fuse the payloads of different transfer symbols.

Scan inputs are limited to 40 megapixels. Registration and equalization can mitigate some channel displacement and sampling interference, but cannot reconstruct information already lost to defocus, display rescaling, clipping, or severe aliasing. They are not a general correction for arbitrary lens distortion or moiré. Reported raster dimensions and bits per pixel do not establish a reliable physical camera footprint.

## 10. CWT transfer envelope

CWT carries one source or repair shard as the application payload of an ordinary CW Luma 10 image. It does not alter the optical alphabet or ECC. A transfer may begin at any symbol; there is no separate start image that must be captured first.

### Header and body

Every envelope consists of this 64-byte header followed by exactly `S` bytes of shard data. All multibyte integers are unsigned little-endian.

| Offset | Bytes | Field | Constraint |
| ---: | ---: | --- | --- |
| 0 | 4 | Magic | ASCII `CWT` followed by `0x00` (`43 57 54 00`) |
| 4 | 16 | Transfer identity | Common to every symbol in the transfer |
| 20 | 2 | Global symbol index `i` (`u16`) | Source-or-repair position defined below |
| 22 | 2 | Source chunk count `N` (`u16`) | Exactly `ceil(L/S)` |
| 24 | 2 | Chunk size `S` (`u16`) | 10,240 or 20,480 bytes |
| 26 | 4 | Original file length `L` (`u32`) | 1 through 33,554,432 bytes |
| 30 | 32 | File SHA-256 | Digest of the original `L` bytes, before padding |
| 62 | 1 | Nominal source group size `G` | 20 or 60 |
| 63 | 1 | Repair shards per group | Exactly 4 |
| 64 | `S` | Shard body | Full-size source or repair data |

The generator creates a random 16-byte identity; the in-memory API also permits an application-supplied identity. This field distinguishes transfers and has no authentication semantics.

Source shard `j` contains file bytes `[j*S, min((j+1)*S,L))`, padded with zero bytes to `S`. Only the final source can need padding. Nonzero source padding and any envelope length other than `64+S` are rejected. Repair bodies always occupy `S` bytes and are not trimmed or interpreted as file padding.

The sender uses uncompressed optical packets for transfer envelopes. Thus every source and repair image in a transfer has the same payload length, grid, and raster dimensions, including the final source. Original file bytes are preserved without media transcoding.

For Low ECC at eight pixels per cell:

| Source chunk size | Optical payload including CWT header | Grid | Complete raster |
| --- | ---: | ---: | --- |
| 10 KiB | 10,304 bytes | 108×108 cells | 896×896 pixels |
| 20 KiB | 20,544 bytes | 149×149 cells | 1224×1224 pixels |

### Group indexing

For file length $`L`$, chunk size $`S`$, and nominal source group size $`G`$, the source, group, and transmitted-symbol counts are:

```math
N=\left\lceil\frac{L}{S}\right\rceil,\qquad
Q=\left\lceil\frac{N}{G}\right\rceil,\qquad
T=N+4Q.
```

Each group emits its sources followed immediately by its four repairs. All but the last group contain $`G`$ sources; the final group contains only the remaining $`k`$ sources.

For global index `i`:

```text
group        = floor(i / (G+4))
local        = i mod (G+4)
source_start = group*G
k            = min(G, N-source_start)

if local < k: source index = source_start + local
otherwise:   repair index = local-k, in 0..3
```

Require `i<T`, a nonempty source group, and `local<k+4`. The final group's repairs follow its actual `k` sources; there is no gap for unused nominal source positions.

For example, `N=23` and `G=20` produce 31 symbols:

```text
global  0..19  → source 0..19
       20..23  → group 0 repairs 0..3
       24..26  → source 20..22
       27..30  → group 1 repairs 0..3
```

### Repair arithmetic

For each byte offset `b=0..S−1` in a group, collect the column:

```text
source[0][b], source[1][b], …, source[k−1][b]
```

Encode this $`k`$-byte message systematically with four RS parity bytes, using the same $`\mathrm{GF}(256)`$, byte order, and generator convention as section 5. The generator for four parity bytes is:

```math
G_4(x)=x^4+15x^3+54x^2+120x+64,
```

with coefficients interpreted as $`\mathrm{GF}(256)`$ bytes. The resulting $`k+4`$ bytes become that column of the source and repair shards. Neither CWT headers nor optical packet headers participate in this outer code.

Equivalently, this is a shortened RS(255,251) code with `251−k` leading zero data bytes omitted. Any `k` distinct correct shards from the group's `k+4` positions recover its missing source shards. At most four positions may be missing, and repair shards lost to the camera consume that same erasure budget. A partial final group still receives four repairs.

Received shards have already passed optical packet validation. Transfer repair treats missing shards as erasures; it does not attempt unknown-error correction on received shards. Reconstruction checks consistency against every supplied repair shard. With only the minimum number of independent equations, an incorrect shard can still produce a consistent reconstruction, which is why whole-file SHA-256 remains mandatory.

### Receiver state and completion

The receiver retains sources and repairs by index, independent of arrival order, including across playback loops. It can start with a repair symbol or a source in the middle of the file. Whenever a group has enough distinct validated shards, it attempts recovery without waiting for the missing source images to return.

All envelopes must agree on transfer identity, source count, chunk size, file length, digest, and group size. An identical duplicate is ignored. A conflicting duplicate, inconsistent metadata, or foreign transfer is rejected; switching transfers requires a reset. Recovered sources must also have canonical zero tail padding. Invalid repair results must not be committed as new received data.

Once every source is available, concatenate sources in file order, discard only the final zero padding, and verify exactly `L` bytes against the advertised SHA-256. Deliver the file only after this succeeds. A digest failure requires reset rather than reporting a completed file.

CRC-32, parity consistency, and SHA-256 detect corruption; none authenticates the sender because the sender supplies the checksum and digest. Applications requiring authenticity must protect their payload separately.

### Playback and rate accounting

The CLI defaults to groups of 20; `--repair-group 60` trades fewer repair images for a larger recovery group. The player loops over the generated symbols. A repair hold of 1× displays each symbol for one slot; a higher hold extends only repair slots. No receiver acknowledgement channel is required.

With playback rate $`f`$ slots/second and a 1× hold, the useful offered rate for one complete loop is:

```math
R_{\mathrm{offered}}=\frac{8Lf}{T}\ \text{bits/second}.
```

For full groups, ignoring the short-tail effect, this approaches:

```math
R_{\mathrm{offered}}\approx8Sf\frac{G}{G+4}.
```

For 20 KiB source chunks, 12 slots/second, and groups of 20, the nominal useful rate is:

```math
8(20480)(12)\frac{20}{24}
=1{,}638{,}400\ \text{bits/second}
=1.6384\ \text{Mbit/s}.
```

This is below $`8Sf`$ because repair slots carry redundancy. Repeated frames, missed captures, reacquisition, and additional loops further affect delivered goodput.

The app's useful-byte counters exclude headers, padding, duplicate data, and repair bodies. Reconstructed source bytes count once. Completed-file goodput divides `8*L` by the interval from receipt of the first subsequently accepted source **or repair** camera buffer through full-file validation. That interval includes the first decode, intervening failures, duplicate reads, repairs, and reassembly; initial camera acquisition before the first accepted symbol is excluded. See [benchmarking](docs/benchmarking.md) for measurement controls.

## 11. Implementation map

| Responsibility | Source |
| --- | --- |
| Cell alphabet and pilot sequence | [src/luma.rs](src/luma.rs) |
| Reservations and traversal bounds | [src/integrated_layout.rs](src/integrated_layout.rs) |
| Packet, optical ECC, interleaving, calibration | [src/codec.rs](src/codec.rs) |
| Finders, metadata, framed raster, scanning | [src/scanner.rs](src/scanner.rs) |
| Channel registration and equalization | [src/grid_refine.rs](src/grid_refine.rs), [src/spatial_equalizer.rs](src/spatial_equalizer.rs) |
| Lossless compression envelope | [src/compression.rs](src/compression.rs) |
| CWT headers and group construction | [src/transfer.rs](src/transfer.rs) |
| Cross-image erasure repair | [src/transfer_repair.rs](src/transfer_repair.rs) |
| iPhone transfer assembly and validation | [TransferReceiver.swift](ios/ChromaWeave/TransferReceiver.swift) |

The [architecture guide](docs/architecture.md) describes API and platform boundaries. The compatibility fixtures provide exact expected bytes and pixels for independent implementations; decoder heuristics and performance measurements are not part of the wire contract.
