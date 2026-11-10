# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = ["torch", "numpy"]
# ///
"""Trains the value net of src/solver/net.rs on samples files from `warden-cli samples`.

The net is inputs -> h1 -> h2 -> 1 with clamp(x, 0, 1) after each hidden layer and a
linear output. The weights file it writes is read by `warden-cli net` and `Net::from_bytes`.
"""

import argparse
import struct
import time

import numpy as np
import torch


def read_samples(path):
    """The samples file at `path` as (inputs, labels, offsets, indices, values) arrays."""
    data = open(path, "rb").read()
    if data[:4] != b"WSMP":
        raise SystemExit(f"{path}: not a samples file")
    inputs, count, nnz = struct.unpack_from("<III", data, 4)
    at = 16
    labels = np.frombuffer(data, "<f4", count, at).astype(np.float32)
    at += count * 4
    offsets = np.frombuffer(data, "<u4", count + 1, at).astype(np.int64)
    at += (count + 1) * 4
    indices = np.frombuffer(data, "<u2", nnz, at).astype(np.int64)
    at += nnz * 2
    values = np.frombuffer(data, "<f4", nnz, at).astype(np.float32)
    at += nnz * 4
    if at != len(data):
        raise SystemExit(f"{path}: {len(data)} bytes, expected {at}")
    return inputs, labels, offsets, indices, values


def join(sets):
    """Several sample sets with the same inputs as one."""
    inputs = sets[0][0]
    for other in sets[1:]:
        if other[0] != inputs:
            raise SystemExit(f"the train files disagree on the inputs: {inputs} and {other[0]}")
    labels = np.concatenate([s[1] for s in sets])
    starts = np.cumsum([0] + [len(s[3]) for s in sets[:-1]])
    offsets = np.concatenate([s[2][:-1] + start for s, start in zip(sets, starts)] + [[sum(len(s[3]) for s in sets)]])
    return inputs, labels, offsets, np.concatenate([s[3] for s in sets]), np.concatenate([s[4] for s in sets])


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
    _, labels, offsets, indices, values = set_
    starts = offsets[rows]
    lens = offsets[rows + 1] - starts
    bag = np.zeros(len(rows), np.int64)
    np.cumsum(lens[:-1], out=bag[1:])
    flat = np.arange(lens.sum()) + np.repeat(starts - bag, lens)
    return (
        torch.from_numpy(indices[flat]),
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
    parser.add_argument("--val")
    parser.add_argument("--out", required=True)
    parser.add_argument("--h1", type=int, default=128)
    parser.add_argument("--h2", type=int, default=32)
    parser.add_argument("--epochs", type=int, default=8)
    parser.add_argument("--batch", type=int, default=1024)
    parser.add_argument("--lr", type=float, default=1e-3)
    parser.add_argument("--seed", type=int, default=1)
    args = parser.parse_args()

    torch.manual_seed(args.seed)
    torch.set_num_threads(4)
    rng = np.random.default_rng(args.seed)
    train = join([read_samples(path) for path in args.train])
    val = read_samples(args.val) if args.val else None
    inputs = train[0]
    if val is not None and val[0] != inputs:
        raise SystemExit(f"{args.val}: {val[0]} inputs, the train files have {inputs}")
    count = len(train[1])
    print(f"{count} train samples" + (f", {len(val[1])} val samples" if val else ""))
    mean = float(train[1].mean())
    if val is not None:
        print(f"constant predictor val rmse {rmse(np.full(len(val[1]), mean), val[1]):.6f}")

    model = Net(inputs, args.h1, args.h2)
    with torch.no_grad():
        model.out.bias.fill_(mean)
    opt = torch.optim.Adam(model.parameters(), lr=args.lr)
    loss_fn = torch.nn.MSELoss()
    for epoch in range(1, args.epochs + 1):
        clock = time.time()
        model.train()
        perm = rng.permutation(count)
        for at in range(0, count, args.batch):
            i, o, w, y = batch(train, perm[at : at + args.batch])
            opt.zero_grad()
            loss = loss_fn(model(i, o, w), y)
            loss.backward()
            opt.step()
        model.eval()
        line = f"epoch {epoch} train rmse {rmse(predict(model, train, 8192), train[1]):.6f}"
        if val is not None:
            line += f" val rmse {rmse(predict(model, val, 8192), val[1]):.6f}"
        print(f"{line} {time.time() - clock:.1f} s")

    write_weights(model, args.out, inputs, args.h1, args.h2)
    first = predict(model, val if val is not None else train, 8192)[:5]
    print("first: " + " ".join(f"{v:.6f}" for v in first))


if __name__ == "__main__":
    main()
