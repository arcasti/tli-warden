# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["torch", "numpy"]
# ///
"""Trains the value net of src/solver/net.rs on samples files from `warden-cli samples`.

The net is inputs -> h1 -> h2 -> 1 with clamp(x, 0, 1) after each hidden layer and a
linear output. The weights file it writes is read by `warden-cli net` and `Net::from_bytes`.
The loss is the squared error on each sample plus `--pair-weight` times the squared error on
values and labels centred on their sibling group's mean, so the net learns the differences
between the options of one decision.

The train files are held in memory `--window` files at a time: each epoch shuffles the
order of the files and trains on each window in turn, on shuffled whole groups. With no
more train files than `--window`, they are loaded once and kept for every epoch. The
train rmse printed after each epoch is measured during the epoch, on each batch before
its update, so it is taken while the weights are still moving. `--threads` sets the
number of torch threads.
"""

import argparse
import os
import struct
import time

import numpy as np
import torch


def read_header(f, path):
    """The (inputs, count, nnz) of the samples file open as `f`, which is left at the labels."""
    head = f.read(16)
    if head[:4] != b"WSM2":
        raise SystemExit(f"{path}: not a samples file of this version (expected magic WSM2)")
    return struct.unpack_from("<III", head, 4)


def read_labels(path):
    """The inputs and the labels of the samples file at `path`, reading nothing else."""
    with open(path, "rb") as f:
        inputs, count, _ = read_header(f, path)
        labels = np.fromfile(f, "<f4", count)
    if len(labels) != count:
        raise SystemExit(f"{path}: {len(labels)} labels, expected {count}")
    return inputs, labels


def read_samples(path):
    """The samples file at `path` as (inputs, labels, groups, offsets, indices, values), in the
    types the file stores: float32 labels and values, uint32 groups and offsets, uint16 indices."""
    with open(path, "rb") as f:
        inputs, count, nnz = read_header(f, path)
        expected = 16 + count * 4 * 3 + 4 + nnz * 6
        if os.fstat(f.fileno()).st_size != expected:
            raise SystemExit(f"{path}: {os.fstat(f.fileno()).st_size} bytes, expected {expected}")
        labels = np.fromfile(f, "<f4", count)
        groups = np.fromfile(f, "<u4", count)
        offsets = np.fromfile(f, "<u4", count + 1)
        indices = np.fromfile(f, "<u2", nnz)
        values = np.fromfile(f, "<f4", nnz)
    if np.any(groups[1:] < groups[:-1]):
        raise SystemExit(f"{path}: the groups are not in order")
    return inputs, labels, groups, offsets, indices, values


def join(sets):
    """Several sample sets as one, with the inputs of the widest; each file's group ids come after the
    previous file's. A file with fewer inputs uses the first of them: inputs are only ever added at the end.
    Groups and offsets become int64; indices and values keep their types."""
    inputs = max(s[0] for s in sets)
    labels = np.concatenate([s[1] for s in sets])
    shifts = np.cumsum([0] + [int(s[2][-1]) + 1 if len(s[2]) else 0 for s in sets[:-1]])
    groups = np.concatenate([s[2].astype(np.int64) + shift for s, shift in zip(sets, shifts)])
    starts = np.cumsum([0] + [len(s[4]) for s in sets[:-1]])
    ends = [s[3][:-1].astype(np.int64) + start for s, start in zip(sets, starts)]
    offsets = np.concatenate(ends + [np.array([sum(len(s[4]) for s in sets)], np.int64)])
    return inputs, labels, groups, offsets, np.concatenate([s[4] for s in sets]), np.concatenate([s[5] for s in sets])


def group_ranges(groups):
    """The [start, end) sample ranges of the groups, which are contiguous runs of equal ids."""
    cuts = np.flatnonzero(np.diff(groups)) + 1
    starts = np.concatenate([[0], cuts]).astype(np.int64)
    ends = np.concatenate([cuts, [len(groups)]]).astype(np.int64)
    return starts, ends


def pair_rmse(pred, labels, groups):
    """For the members of groups of 2+: the rmse of the value minus its group's mean against the
    label minus its group's mean, and the same for a value of 0; None for a file without such groups."""
    _, ids, sizes = np.unique(groups, return_inverse=True, return_counts=True)
    member = sizes[ids] >= 2
    if not member.any():
        return None
    pred, labels = pred.astype(np.float64), labels.astype(np.float64)
    cv = pred - (np.bincount(ids, pred) / sizes)[ids]
    cl = labels - (np.bincount(ids, labels) / sizes)[ids]
    return float(np.sqrt(np.mean((cv - cl)[member] ** 2))), float(np.sqrt(np.mean(cl[member] ** 2)))


class Net(torch.nn.Module):
    def __init__(self, inputs, h1, h2):
        super().__init__()
        self.first = torch.nn.EmbeddingBag(inputs, h1, mode="sum")
        self.b1 = torch.nn.Parameter(torch.zeros(h1))
        self.second = torch.nn.Linear(h1, h2)
        self.out = torch.nn.Linear(h2, 1)

    def forward(self, indices, offsets, weights):
        a1 = torch.clamp(self.first(indices, offsets, per_sample_weights=weights) + self.b1, 0, 1)
        a2 = torch.clamp(self.second(a1), 0, 1)
        return self.out(a2).squeeze(1)


def batch(set_, rows):
    """The EmbeddingBag inputs and the labels of samples `rows` of `set_`."""
    _, labels, _, offsets, indices, values = set_
    starts = offsets[rows].astype(np.int64)
    lens = offsets[rows + 1].astype(np.int64) - starts
    bag = np.zeros(len(rows), np.int64)
    np.cumsum(lens[:-1], out=bag[1:])
    flat = np.arange(lens.sum()) + np.repeat(starts - bag, lens)
    return (
        torch.from_numpy(indices[flat].astype(np.int64)),
        torch.from_numpy(bag),
        torch.from_numpy(values[flat]),
        torch.from_numpy(labels[rows]),
    )


def predict(model, set_, size):
    """The model's values for every sample of `set_`."""
    count = len(set_[1])
    out = []
    with torch.no_grad():
        for at in range(0, count, size):
            i, o, w, _ = batch(set_, np.arange(at, min(at + size, count)))
            out.append(model(i, o, w).numpy())
    return np.concatenate(out) if out else np.zeros(0, np.float32)


def rmse(pred, labels):
    return float(np.sqrt(np.mean((pred.astype(np.float64) - labels) ** 2)))


def write_weights(model, path, inputs, h1, h2):
    """Writes the weights file layout of src/solver/net.rs."""
    parts = [
        model.first.weight.detach().numpy(),  # [inputs, h1]
        model.b1.detach().numpy(),
        model.second.weight.detach().numpy().T,  # Linear is [out, in]; the file wants [in, out]
        model.second.bias.detach().numpy(),
        model.out.weight.detach().numpy().reshape(-1),
        model.out.bias.detach().numpy(),
    ]
    with open(path, "wb") as f:
        f.write(b"WNET" + struct.pack("<III", inputs, h1, h2))
        for p in parts:
            f.write(np.ascontiguousarray(p, dtype="<f4").tobytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--train", required=True, nargs="+", help="one or more samples files, trained on together")
    parser.add_argument("--val", nargs="*", default=[], help="samples files reported on after each epoch")
    parser.add_argument("--out", required=True)
    parser.add_argument("--h1", type=int, default=128)
    parser.add_argument("--h2", type=int, default=32)
    parser.add_argument("--epochs", type=int, default=8)
    parser.add_argument("--batch", type=int, default=1024, help="least samples per batch; a batch holds whole groups")
    parser.add_argument("--lr", type=float, default=1e-3)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument(
        "--pair-weight", type=float, default=1.0, help="weight of the error on values centred on their group's mean"
    )
    parser.add_argument("--window", type=int, default=8, help="train files held in memory at a time")
    parser.add_argument("--threads", type=int, default=4, help="torch threads")
    args = parser.parse_args()

    torch.manual_seed(args.seed)
    torch.set_num_threads(args.threads)
    rng = np.random.default_rng(args.seed)
    # A pass over the labels alone gives the sample count and the mean for the output bias.
    # The net takes the inputs of the widest train file (see `join`).
    inputs, count, total = 0, 0, 0.0
    for path in args.train:
        file_inputs, labels = read_labels(path)
        inputs, count, total = max(inputs, file_inputs), count + len(labels), total + float(labels.sum(dtype=np.float64))
    del labels
    windowed = len(args.train) > args.window
    train = None if windowed else join([read_samples(path) for path in args.train])
    vals = [(path, read_samples(path)) for path in args.val]
    for path, val in vals:
        if val[0] > inputs:
            raise SystemExit(f"{path}: {val[0]} inputs, the train files have {inputs}")
    print(
        f"{count} train samples in {len(args.train)} files, window {args.window}"
        + "".join(f", {len(val[1])} val samples in {path}" for path, val in vals),
        flush=True,
    )
    mean = total / count
    for path, val in vals:
        print(f"{path}: constant predictor val rmse {rmse(np.full(len(val[1]), mean), val[1]):.6f}", flush=True)

    model = Net(inputs, args.h1, args.h2)
    with torch.no_grad():
        model.out.bias.fill_(mean)
    opt = torch.optim.Adam(model.parameters(), lr=args.lr)
    for epoch in range(1, args.epochs + 1):
        clock = time.time()
        model.train()
        if windowed:
            files = [args.train[j] for j in rng.permutation(len(args.train))]
            windows = [files[k : k + args.window] for k in range(0, len(files), args.window)]
        else:
            windows = [None]
        squared, seen = 0.0, 0
        for window in windows:
            if window is not None:
                train = None  # frees the previous window before the next one is read
                train = join([read_samples(path) for path in window])
            starts, ends = group_ranges(train[2])
            order = rng.permutation(len(starts))
            first_rows, sizes = starts[order], ends[order] - starts[order]
            cum = np.concatenate([[0], np.cumsum(sizes)])
            g = 0
            while g < len(order):
                # Whole groups until the batch holds at least --batch samples.
                e = min(int(np.searchsorted(cum, cum[g] + args.batch, side="left")), len(order))
                e = max(e, g + 1)
                n = sizes[g:e]
                rows = np.repeat(first_rows[g:e] - (cum[g:e] - cum[g]), n) + np.arange(cum[e] - cum[g])
                gidx = torch.from_numpy(np.repeat(np.arange(e - g), n))
                i, o, w, y = batch(train, rows)
                opt.zero_grad()
                v = model(i, o, w)
                loss = torch.mean((v - y) ** 2)
                squared += float(((v.detach().double() - y.double()) ** 2).sum())
                seen += len(rows)
                if args.pair_weight:
                    k = torch.from_numpy(n.astype(np.float32))
                    cv = v - (torch.zeros(e - g).index_add_(0, gidx, v) / k)[gidx]
                    cl = y - (torch.zeros(e - g).index_add_(0, gidx, y) / k)[gidx]
                    loss = loss + args.pair_weight * torch.mean((cv - cl) ** 2)
                loss.backward()
                opt.step()
                g = e
        model.eval()
        print(f"epoch {epoch} train rmse {np.sqrt(squared / seen):.6f} {time.time() - clock:.1f} s", flush=True)
        for path, val in vals:
            pred = predict(model, val, 1024)
            line = f"  {path}: val rmse {rmse(pred, val[1]):.6f}"
            pair = pair_rmse(pred, val[1], val[2])
            if pair is not None:
                line += f" pair rmse {pair[0]:.6f} pair rmse of no difference {pair[1]:.6f}"
            print(line, flush=True)

    write_weights(model, args.out, inputs, args.h1, args.h2)
    first = predict(model, vals[0][1] if vals else train, 1024)[:5]
    print("first: " + " ".join(f"{v:.6f}" for v in first), flush=True)


if __name__ == "__main__":
    main()
