# Content use

Some vendors say that what you send them, and what their models answer, may be
used to train or improve their models. On those routes crucible asks you once
before anything is sent, and remembers your answer.

## What is asked, and when

The first time a message would go on such a route, a panel stands in place of
the prompt box. It names the route, says what the vendor's own terms say in
English, and names the page and the day it was read. Nothing has been sent at
that point: not the message, not a web search, not a sign-in, not the renewal
of a token.

- **Use it anyway** sends the message and writes the route into your own
  configuration file, so the question is not asked again, in this run or a
  later one.
- **Go back**, or Escape, sends nothing and leaves your message in the prompt
  box.

The same question is asked when you choose such a route in `/login`, or a
model of one in `/model`, before the choice is taken. A yes given in `/login`
is written down once the credential is stored; a sign-in that fails or is
left writes nothing down.

A window too short for the whole panel says so and sends nothing:

```text
! this route needs an answer first; make the window taller and send again
```

A run with no terminal to ask on ends before sending, with what the vendor's
terms say and `Nothing was sent; answer it once in a terminal.` on standard
error, and exit status 1. A client of the application with no terminal is put
the same question as a pending `warning`, answered with an `accepted` or a
`declined` decision naming it.

Behind the question, every request crucible sends to a host of such a route is
held until the route has its yes, however the route was reached: `/login`,
`/model`, a key in the environment, the configuration file, `--model`, or a
session picked up with `--continue` or `--resume`.

## Where the answer is kept

Each yes is a route name in `contentUse.accepted`, in the configuration file
in your home directory:

```json
{ "contentUse": { "accepted": ["key:google"] } }
```

Only that file is read for it. Either project file that sets
`contentUse.accepted` stops crucible before it draws anything, naming the file
and the key, as every key a checkout may not set does.

A yes is taken out of the file before a credential of its route is removed or
replaced by one of another row: `/logout`, a sign-in or key on another row of
the same provider, and a second credential taken out at a start. With it go
the yes of every model of that provider and the yes of the route its
`baseUrl` answers for. A new account on the route has agreed to nothing, so it
is asked again. A key changed in the environment moves nothing. To take a yes
back yourself, delete its name from the list.

## The routes asked about

A route is a `/login` row, named by its list and the name its credential is
stored under, or a Kimi open platform address, named by its host. The sentence
is what the panel says; the caution is what the row says in `/login`. Each
quote is the vendor's own, in the language it was written in.

| Route | Name | Condition | Panel sentence | Caution | Vendor's words | Way out | Source | Read |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| OpenAI subscription | `subscription:openai` | On Free, Plus and Pro | On Free, Plus and Pro, OpenAI may use what you send to train its models. To stop it, turn off Improve the model for everyone under Settings, Data controls in ChatGPT, or choose Do not train on my content in its Privacy Portal; that covers only what you send afterwards. | may train on what is sent | "When you use our services for individuals, such as ChatGPT and Codex, we may use your content to train our models. [...] To opt out, turn off Improve the model for everyone under Settings > Data controls in ChatGPT, or select Do not train on my content in our Privacy Portal." "If you are on a ChatGPT Plus, ChatGPT Pro or ChatGPT Free plan on a personal workspace, data sharing is enabled for you by default" | Settings, Data controls in ChatGPT, or the Privacy Portal | [OpenAI Help Center, archived copy](https://help.openai.com/en/articles/5722486-how-your-data-is-used-to-improve-model-performance) | 28 Sep 2026 |
| Kimi Code · kimi.ai subscription | `subscription:moonshot@kimi.ai` | | Kimi may use what you send to train its models. To stop it, contact Kimi as its terms say; that covers only what you send afterwards. | may train on what is sent | "subject to your training opt-out below, use Content to train, evaluate, and improve the Services." "You can opt out of allowing your Content to be used to train the Services by contacting us [...] Opt-out applies prospectively only" | Contact Kimi, for what is sent afterwards | [kimi.ai terms of service](https://www.kimi.ai/user/agreement/modelUse?version=v2) | 30 Sep 2026 |
| Kimi Code · kimi.com subscription | `subscription:moonshot` | | Kimi may use what you send, and what it answers, to improve its models. To keep it out of training, contact Kimi as its terms say. | may use what is sent | "为了提升您使用本服务的体验，您授予我们一项免费的使用权，以在法律允许的范围内将您输入输出之内容及反馈用于模型服务优化。如您不希望您的内容被用于模型训练，您可以通过本协议所载的联系方式联系我们。" | Contact Kimi | [kimi.com user agreement](https://www.kimi.com/user/agreement/modelUse?version=v2) | 30 Sep 2026 |
| Google key | `key:google` | On unpaid quota | On unpaid quota, Google uses what you send and what it answers to improve its products and machine learning technologies. In the EEA, Switzerland and the UK the paid terms apply instead. | uses what is sent | "When you use Unpaid Services, including, for example, Google AI Studio and the unpaid quota on Gemini API, Google uses the content you submit to the Services and any generated responses to provide, improve, and develop Google products and services and machine learning technologies" "If you're in the European Economic Area, Switzerland, or the United Kingdom, the terms under "How Google uses Your Data" in "Paid Services" apply to all Services, including Google AI Studio and unpaid quota in the Gemini API" | None given | [Gemini API terms](https://ai.google.dev/gemini-api/terms) | 30 Sep 2026 |
| MoonshotAI · kimi.ai key | `key:moonshot@kimi.ai` | | Kimi may use what you send to train its models. To stop it, contact Kimi as its terms say; that covers only what you send afterwards. | may train on what is sent | As the kimi.ai subscription: a Kimi Code Console key is the same service. | Contact Kimi, for what is sent afterwards | [kimi.ai terms of service](https://www.kimi.ai/user/agreement/modelUse?version=v2) | 30 Sep 2026 |
| MoonshotAI · kimi.com key | `key:moonshot` | | Kimi may use what you send, and what it answers, to improve its models. To keep it out of training, contact Kimi as its terms say. | may use what is sent | As the kimi.com subscription: a Kimi Code Console key is the same service. | Contact Kimi | [kimi.com user agreement](https://www.kimi.com/user/agreement/modelUse?version=v2) | 30 Sep 2026 |
| Kimi open platform, `api.moonshot.ai` | `api.moonshot.ai` | | Moonshot may use what you send to develop and improve its services, and to train its models, unless you agree otherwise with it in writing. | may train on what is sent | "We may use Content to provide, maintain, develop, support, and improve the Services [...] Customer who requires restrictions on the use of Customer Content for training or improving Moonshot AI models may contact Moonshot AI to discuss available enterprise arrangements or separate written agreements. Unless otherwise expressly agreed in writing, Customer Content may be used for the foregoing purposes." | A written agreement with Moonshot | [Kimi open platform terms](https://platform.kimi.ai/docs/agreement/modeluse) | 30 Sep 2026 |
| Kimi open platform, `api.moonshot.cn` | `api.moonshot.cn` | | Moonshot may use what you send, and what it answers, to improve its models. | may use what is sent | "为了提升您使用本服务的体验，您授予我们一项免费的使用权，以在法律允许的范围内将您输入输出之内容及反馈用于模型服务优化。" | None given | [Kimi open platform terms, platform.kimi.com](https://platform.kimi.com/docs/agreement/modeluse) | 30 Sep 2026 |

The OpenAI help pages refuse automated readers, so that row rests on the
Internet Archive's copies of 28 and 14 September 2026, the date shown being the
later.

These are not asked about, because their vendors' terms say what is sent is
not used for training unless you agree: the `OpenAI` and `Anthropic` key rows.
A key from a provider's variable belongs to one row of it: for MoonshotAI, the
kimi.com key row.

## A `baseUrl` crucible recognises

A `baseUrl` is warned only at an address crucible documents. It is recognised
when its scheme is the address's, its host is the address's (case, a trailing
dot and the default port written out do not matter), and its path is the
address's path or lies under it by whole segments, read as the path it names:
percent-encoding decoded and `.` and `..` segments resolved. Every other `baseUrl`,
including `https://chatgpt.com/backend-api/codex/responses`, where no key row
stands, is not asked about.

| Address | Route it answers for |
| --- | --- |
| `https://api.anthropic.com/v1` | `key:anthropic`, not asked about |
| `https://generativelanguage.googleapis.com/v1beta` | `key:google` |
| `https://api.kimi.ai/coding/v1` | `key:moonshot@kimi.ai` |
| `https://api.kimi.com/coding/v1` | `key:moonshot` |
| `https://api.openai.com/v1` | `key:openai`, not asked about |
| `https://api.moonshot.ai/v1` | `api.moonshot.ai` |
| `https://api.moonshot.cn/v1` | `api.moonshot.cn` |

## Rolling back to 0.43.3

0.43.3 does not know `contentUse`, and stops before drawing anything on a file
that has it, with a line that begins:

```text
crucible: <home>/config.json: contentUse is not a setting crucible has at line <n>, column <m>
```

Delete the `contentUse` block from the configuration file in your home
directory before running 0.43.3.
